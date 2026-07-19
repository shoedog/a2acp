//! Owner-private evidence index and retention primitives for R3d3.
//!
//! This module is deliberately effect-local: it persists only under an injected owner-lock
//! capability. R3d5 remains the sole production root initializer and activation owner.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write as _;
use std::os::fd::AsRawFd as _;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule_schema::{
    relative_evidence_path, ColdStorageBindingV1, EvidenceClassV1, EvidenceIndexEntryV1,
    EvidenceIndexV1, OptionalRelativeEvidencePathV1, OptionalSha256V1, RelativeEvidencePathV1,
    ValidateRecord,
};
use crate::compatibility_schedule_state::EvidenceStateCapability;
use crate::{local_file, BoxError};

pub(super) const DAY_MS: i64 = 86_400_000;
const MAX_EVIDENCE_ITEMS: usize = 256;
const MAX_STATE_RECORD_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STATE_GENERATIONS: usize = 10_000;
const STATE_FILE_MODE: u32 = 0o600;
const STATE_PREFIX: &str = "evidence-state.";
const HOT_TOTAL_CAP_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const HOT_STATE_CAP_BYTES: u64 = 1024 * 1024 * 1024;
const HOT_SCRATCH_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const HOT_SEALED_CAP_BYTES: u64 = 5 * 1024 * 1024 * 1024;

fn require_sha256(label: &str, value: &str) -> Result<(), BoxError> {
    if !local_file::valid_sha256(value) || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(format!("schedule evidence: {label} is not lowercase SHA-256").into());
    }
    Ok(())
}

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
pub(super) struct EvidenceRetentionRequestV1 {
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) terminal_at_ms: i64,
    pub(super) case_minimum_days: u32,
    pub(super) release_retain_until_ms: Option<i64>,
    pub(super) pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RetentionDecisionV1 {
    pub(super) full_retain_until_ms: i64,
    pub(super) compact_retain_until_ms: i64,
    pub(super) hot_retain_until_ms: i64,
}

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
pub(super) struct IndexedEvidenceV1 {
    pub(super) evidence_id: String,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) full_evidence_sha256: String,
    pub(super) compact_record_sha256: String,
    pub(super) archive_bytes: u64,
    pub(super) manifest_bytes: u64,
    pub(super) compact_record_bytes: u64,
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
    fn sealed_hot_bytes(&self) -> Result<u64, BoxError> {
        self.archive_bytes
            .checked_add(self.manifest_bytes)
            .ok_or_else(|| "schedule evidence: indexed sealed byte total overflow".into())
    }

    fn total_indexed_bytes(&self) -> Result<u64, BoxError> {
        self.sealed_hot_bytes()?
            .checked_add(self.compact_record_bytes)
            .ok_or_else(|| "schedule evidence: indexed hot byte total overflow".into())
    }

    fn immutable_eq(&self, other: &Self) -> bool {
        self.evidence_id == other.evidence_id
            && self.evidence_class == other.evidence_class
            && self.full_evidence_sha256 == other.full_evidence_sha256
            && self.compact_record_sha256 == other.compact_record_sha256
            && self.archive_bytes == other.archive_bytes
            && self.manifest_bytes == other.manifest_bytes
            && self.compact_record_bytes == other.compact_record_bytes
            && self.hot_path == other.hot_path
            && self.terminal_at_ms == other.terminal_at_ms
            && self.case_minimum_days == other.case_minimum_days
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum PinLifecycleV1 {
    Active,
    Released { released_at_ms: i64, reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidencePinV1 {
    pub(super) pin_id: String,
    pub(super) evidence_id: String,
    pub(super) reason: String,
    pub(super) created_at_ms: i64,
    pub(super) lifecycle: PinLifecycleV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TombstoneLifecycleV1 {
    Pending,
    FullEvidenceUnlinked { unlinked_at_ms: i64 },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceTombstoneV1 {
    pub(super) tombstone_id: String,
    pub(super) evidence_id: String,
    pub(super) full_evidence_sha256: String,
    pub(super) compact_record_sha256: String,
    pub(super) archive_bytes: u64,
    pub(super) manifest_bytes: u64,
    pub(super) compact_record_bytes: u64,
    pub(super) hot_path: RelativeEvidencePathV1,
    pub(super) cold_path: OptionalRelativeEvidencePathV1,
    pub(super) hot_was_present: bool,
    pub(super) full_retain_until_ms: i64,
    pub(super) compact_retain_until_ms: i64,
    pub(super) reason_code: String,
    pub(super) created_at_ms: i64,
    pub(super) lifecycle: TombstoneLifecycleV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceStateModelV1 {
    pub(super) hot_root_sha256: String,
    pub(super) cold_storage: ColdStorageBindingV1,
    pub(super) entries: BTreeMap<String, IndexedEvidenceV1>,
    pub(super) pins: BTreeMap<String, EvidencePinV1>,
    pub(super) tombstones: BTreeMap<String, EvidenceTombstoneV1>,
    pub(super) retired_evidence_ids: BTreeSet<String>,
}

impl EvidenceStateModelV1 {
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
        };
        value.validate()?;
        Ok(value)
    }

    pub(super) fn validate(&self) -> Result<(), BoxError> {
        require_sha256("hot root", &self.hot_root_sha256)?;
        if self.entries.len() > MAX_EVIDENCE_ITEMS
            || self.pins.len() > MAX_EVIDENCE_ITEMS * 4
            || self.tombstones.len() > MAX_EVIDENCE_ITEMS * 4
            || self.retired_evidence_ids.len() > MAX_EVIDENCE_ITEMS * 4
        {
            return Err("schedule evidence: state collections exceed their bounds".into());
        }
        for (id, entry) in &self.entries {
            if id != &entry.evidence_id || self.retired_evidence_ids.contains(id) {
                return Err("schedule evidence: entry key is mismatched or retired".into());
            }
            stable_id("evidence id", id)?;
            require_sha256("full evidence", &entry.full_evidence_sha256)?;
            require_sha256("compact record", &entry.compact_record_sha256)?;
            if entry.archive_bytes == 0
                || entry.manifest_bytes == 0
                || entry.compact_record_bytes == 0
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
            match tombstone.lifecycle {
                TombstoneLifecycleV1::Pending => {
                    let entry = self
                        .entries
                        .get(&tombstone.evidence_id)
                        .ok_or("schedule evidence: pending tombstone has no indexed entry")?;
                    if entry.full_evidence_sha256 != tombstone.full_evidence_sha256
                        || entry.compact_record_sha256 != tombstone.compact_record_sha256
                        || entry.archive_bytes != tombstone.archive_bytes
                        || entry.manifest_bytes != tombstone.manifest_bytes
                        || entry.compact_record_bytes != tombstone.compact_record_bytes
                        || entry.hot_path != tombstone.hot_path
                        || entry.cold_path != tombstone.cold_path
                        || entry.hot_present != tombstone.hot_was_present
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

    pub(super) fn pin(&mut self, pin: EvidencePinV1) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        if candidate.pins.contains_key(&pin.pin_id) || pin.lifecycle != PinLifecycleV1::Active {
            return Err("schedule evidence: pin must be a new active record".into());
        }
        candidate.pins.insert(pin.pin_id.clone(), pin);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

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

    fn has_active_pin(&self, evidence_id: &str) -> bool {
        self.pins
            .values()
            .any(|pin| pin.evidence_id == evidence_id && pin.lifecycle == PinLifecycleV1::Active)
    }

    pub(super) fn begin_tombstone(
        &mut self,
        tombstone_id: &str,
        evidence_id: &str,
        reason_code: &str,
        created_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        if candidate.tombstones.contains_key(tombstone_id) || candidate.has_active_pin(evidence_id)
        {
            return Err("schedule evidence: tombstone id exists or evidence is pinned".into());
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
            full_evidence_sha256: entry.full_evidence_sha256.clone(),
            compact_record_sha256: entry.compact_record_sha256.clone(),
            archive_bytes: entry.archive_bytes,
            manifest_bytes: entry.manifest_bytes,
            compact_record_bytes: entry.compact_record_bytes,
            hot_path: entry.hot_path.clone(),
            cold_path: entry.cold_path.clone(),
            hot_was_present: entry.hot_present,
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

    pub(super) fn complete_tombstone(
        &mut self,
        tombstone_id: &str,
        unlinked_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let tombstone = candidate
            .tombstones
            .get_mut(tombstone_id)
            .ok_or("schedule evidence: tombstone does not exist")?;
        if tombstone.lifecycle != TombstoneLifecycleV1::Pending {
            return Err("schedule evidence: tombstone is already complete".into());
        }
        let evidence_id = tombstone.evidence_id.clone();
        tombstone.lifecycle = TombstoneLifecycleV1::FullEvidenceUnlinked { unlinked_at_ms };
        candidate
            .entries
            .remove(&evidence_id)
            .ok_or("schedule evidence: tombstone target disappeared")?;
        candidate.retired_evidence_ids.insert(evidence_id);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
}

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

fn tombstone_transition_allowed(
    previous: &EvidenceTombstoneV1,
    next: &EvidenceTombstoneV1,
) -> bool {
    previous.tombstone_id == next.tombstone_id
        && previous.evidence_id == next.evidence_id
        && previous.full_evidence_sha256 == next.full_evidence_sha256
        && previous.compact_record_sha256 == next.compact_record_sha256
        && previous.archive_bytes == next.archive_bytes
        && previous.manifest_bytes == next.manifest_bytes
        && previous.compact_record_bytes == next.compact_record_bytes
        && previous.hot_path == next.hot_path
        && previous.cold_path == next.cold_path
        && previous.hot_was_present == next.hot_was_present
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceStateSnapshotV1 {
    pub(super) schema_version: u16,
    pub(super) generation: u64,
    pub(super) previous_record: OptionalSha256V1,
    pub(super) recorded_at_ms: i64,
    pub(super) state: EvidenceStateModelV1,
}

impl EvidenceStateSnapshotV1 {
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

    pub(super) fn project_index(&self) -> Result<EvidenceIndexV1, BoxError> {
        self.validate()?;
        self.state.project_index_at(self.generation)
    }

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
        {
            return Err("schedule evidence: state event postdates its snapshot".into());
        }
        Ok(())
    }
}

pub(super) fn evidence_state_snapshot_sha256(
    value: &EvidenceStateSnapshotV1,
) -> Result<String, BoxError> {
    value.validate()?;
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(local_file::sha256_hex(&bytes))
}

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
        || next.state.cold_storage != previous.state.cold_storage
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

pub(super) struct FileEvidenceJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    next_generation: u64,
    previous_snapshot: EvidenceStateSnapshotV1,
}

pub(super) struct EvidenceJournalOpen<'lock> {
    pub(super) journal: FileEvidenceJournal<'lock>,
    pub(super) snapshot: EvidenceStateSnapshotV1,
    pub(super) snapshot_sha256: String,
}

impl<'lock> FileEvidenceJournal<'lock> {
    fn generation_name(generation: u64) -> String {
        format!("{STATE_PREFIX}{generation:020}.json")
    }

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

    fn persist(&self, snapshot: &EvidenceStateSnapshotV1) -> Result<(), BoxError> {
        snapshot.validate()?;
        let mut bytes = serde_json::to_vec(snapshot)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_STATE_RECORD_BYTES {
            return Err("schedule evidence: state generation exceeds the byte bound".into());
        }
        let name = Self::generation_name(snapshot.generation);
        let mut file = self.directory.create_new_file(
            OsStr::new(&name),
            STATE_FILE_MODE,
            "evidence state generation",
        )?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = self.directory.remove_child(
                OsStr::new(&name),
                false,
                "failed evidence state generation",
            );
            return Err(
                format!("schedule evidence: cannot persist state generation: {error}").into(),
            );
        }
        self.directory.sync()?;
        Ok(())
    }

    pub(super) fn append(
        &mut self,
        state: &EvidenceStateModelV1,
        recorded_at_ms: i64,
    ) -> Result<(EvidenceStateSnapshotV1, String), BoxError> {
        if self.next_generation != self.previous_snapshot.generation.saturating_add(1) {
            return Err("schedule evidence: in-memory journal generation diverged".into());
        }
        let snapshot = self
            .previous_snapshot
            .successor(state.clone(), recorded_at_ms)?;
        validate_evidence_state_transition(&self.previous_snapshot, &snapshot)?;
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

fn lease_name(evidence_id: &str) -> Result<String, BoxError> {
    stable_id("lease evidence id", evidence_id)?;
    Ok(format!(
        "evidence-lease.{}.lock",
        local_file::sha256_hex(evidence_id.as_bytes())
    ))
}

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

pub(super) fn acquire_evidence_read_lease<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    evidence_id: &str,
) -> Result<File, BoxError> {
    acquire_lease(capability, evidence_id, libc::LOCK_SH)
}

pub(super) fn try_acquire_evidence_gc_lease<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    evidence_id: &str,
) -> Result<File, BoxError> {
    acquire_lease(capability, evidence_id, libc::LOCK_EX)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HotAllocationV1 {
    State,
    Scratch,
    Sealed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HotStorageCapsV1 {
    pub(super) total_bytes: u64,
    pub(super) state_bytes: u64,
    pub(super) scratch_bytes: u64,
    pub(super) sealed_bytes: u64,
}

impl HotStorageCapsV1 {
    pub(super) fn approved() -> Self {
        Self {
            total_bytes: HOT_TOTAL_CAP_BYTES,
            state_bytes: HOT_STATE_CAP_BYTES,
            scratch_bytes: HOT_SCRATCH_CAP_BYTES,
            sealed_bytes: HOT_SEALED_CAP_BYTES,
        }
    }

    fn validate(&self) -> Result<(), BoxError> {
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
pub(super) struct HotStorageUsageV1 {
    pub(super) state_bytes: u64,
    pub(super) scratch_bytes: u64,
    pub(super) sealed_bytes: u64,
}

impl HotStorageUsageV1 {
    fn total(self) -> Option<u64> {
        self.state_bytes
            .checked_add(self.scratch_bytes)
            .and_then(|value| value.checked_add(self.sealed_bytes))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_schedule_schema::{
        ColdStorageBindingV1, EvidenceClassV1, OptionalRelativeEvidencePathV1,
        RelativeEvidencePathV1,
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
        let retention = decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::RoutineGreen,
            terminal_at_ms,
            case_minimum_days: 30,
            release_retain_until_ms: None,
            pinned: false,
        })
        .unwrap();
        IndexedEvidenceV1 {
            evidence_id: id.into(),
            evidence_class: EvidenceClassV1::RoutineGreen,
            full_evidence_sha256: digest('a'),
            compact_record_sha256: digest('b'),
            archive_bytes: bytes,
            manifest_bytes: 128,
            compact_record_bytes: 64,
            hot_path: relative(&format!("{id}.tar.gz")),
            cold_path: OptionalRelativeEvidencePathV1::Absent,
            terminal_at_ms,
            case_minimum_days: 30,
            full_retain_until_ms: retention.full_retain_until_ms,
            compact_retain_until_ms: retention.compact_retain_until_ms,
            hot_retain_until_ms: retention.hot_retain_until_ms,
            hot_present: true,
        }
    }

    fn model() -> EvidenceStateModelV1 {
        EvidenceStateModelV1::new(digest('c'), ColdStorageBindingV1::Absent).unwrap()
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
        let mut incident = entry("incident-1", terminal, 512);
        incident.evidence_class = EvidenceClassV1::Incident;
        let retention = decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::Incident,
            terminal_at_ms: terminal,
            case_minimum_days: 0,
            release_retain_until_ms: None,
            pinned: false,
        })
        .unwrap();
        incident.case_minimum_days = 0;
        incident.full_retain_until_ms = retention.full_retain_until_ms;
        incident.compact_retain_until_ms = retention.compact_retain_until_ms;
        incident.hot_retain_until_ms = retention.hot_retain_until_ms;

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
        let mut incident = entry("incident-2", terminal, 512);
        incident.evidence_class = EvidenceClassV1::Incident;
        incident.case_minimum_days = 0;
        incident.full_retain_until_ms = retention.full_retain_until_ms;
        incident.compact_retain_until_ms = retention.compact_retain_until_ms;
        incident.hot_retain_until_ms = retention.hot_retain_until_ms;
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
            .complete_tombstone("tombstone-1", pending.recorded_at_ms - 1)
            .unwrap();
        let next = pending
            .successor(state, pending.recorded_at_ms + 1)
            .unwrap();
        assert!(validate_evidence_state_transition(&pending, &next).is_err());
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
            .complete_tombstone("tombstone-1", previous.recorded_at_ms + 2)
            .unwrap();
        assert!(previous
            .successor(state, previous.recorded_at_ms + 1)
            .is_err());
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
            .complete_tombstone("tombstone-1", previous.recorded_at_ms + 2)
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
            .complete_tombstone("tombstone-1", 20_000_000_003)
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
        let mut evidence = entry("evidence-1", terminal, 512);
        evidence.cold_path = OptionalRelativeEvidencePathV1::RelativePath {
            value: relative("evidence-1.tar.gz"),
        };
        let full_retain_until_ms = evidence.full_retain_until_ms;
        state.insert_entry(evidence).unwrap();

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
        let mut oldest = entry("oldest", 1, 300);
        oldest.cold_path = OptionalRelativeEvidencePathV1::RelativePath {
            value: relative("oldest.tar.gz"),
        };
        let mut pinned = entry("pinned", 2, 400);
        pinned.cold_path = OptionalRelativeEvidencePathV1::RelativePath {
            value: relative("pinned.tar.gz"),
        };
        let mut fresh = entry("fresh", now, 1_000);
        fresh.cold_path = OptionalRelativeEvidencePathV1::RelativePath {
            value: relative("fresh.tar.gz"),
        };
        state.insert_entry(oldest).unwrap();
        state.insert_entry(pinned).unwrap();
        state.insert_entry(fresh).unwrap();
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
            .complete_tombstone("tombstone-1", 20_000_000_001)
            .unwrap();
        let tombstone = state.tombstones.get("tombstone-1").unwrap();
        assert_eq!(tombstone.evidence_id, "evidence-1");
        assert_eq!(tombstone.full_evidence_sha256, digest('a'));
        assert_eq!(tombstone.compact_record_sha256, digest('b'));
        assert_eq!(tombstone.compact_record_bytes, 64);
        assert_eq!(tombstone.cold_path, OptionalRelativeEvidencePathV1::Absent);
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
