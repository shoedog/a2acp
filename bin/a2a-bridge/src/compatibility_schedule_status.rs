//! Durable local schedule status, notification transitions, and read-only rendering.
//!
//! Projection is deliberately closed: every required source is present as healthy, missing,
//! corrupt, or blocked. The latter three always produce a typed degradation. This module has no
//! production notification sink; R3d5 may supply one behind the existing owner capability.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule_authority::authority_status_source_sha256;
use crate::compatibility_schedule_evidence::evidence_status_source_sha256;
use crate::compatibility_schedule_ledger::ledger_status_source_sha256;
use crate::compatibility_schedule_outbox::outbox_status_source_sha256;
use crate::compatibility_schedule_schema::{
    AuthorityStateV1, OneShotCompatibilityStateV1, OptionalAuthorityStatusV1, OptionalRecordRefV1,
    OptionalSha256V1, OptionalTextV1, QuarantineV1, ScheduleCaseLifecycleV1, ScheduleStatusV1,
    StorageStateV1, ValidateRecord,
};
use crate::compatibility_schedule_state::{
    open_production_status_directory_read_only, AdmissionStateCapability, AuthorityStateCapability,
    EvidenceStateCapability, StateQuota,
};
use crate::compatibility_schedule_supervisor::ownership_status_source_sha256;
use crate::compatibility_schedule_transaction::admission_status_source_sha256;
use crate::{local_file, BoxError};

const STATUS_PREFIX: &str = "status-snapshot.";
const NOTIFICATION_PREFIX: &str = "notification.";
const MAX_STATUS_RECORD_BYTES: u64 = 4 * 1024 * 1024;
const MAX_STATUS_GENERATIONS: usize = 100_000;
const STATUS_FILE_MODE: u32 = 0o600;

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, BoxError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_STATUS_RECORD_BYTES {
        return Err("schedule status: record exceeds the byte bound".into());
    }
    Ok(bytes)
}

fn optional_sha256(value: Option<&str>) -> OptionalSha256V1 {
    match value {
        Some(value) => OptionalSha256V1::Sha256 {
            value: value.to_owned(),
        },
        None => OptionalSha256V1::Absent,
    }
}

fn bounded_code(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.len() > 128
        || !matches!(
            value.as_bytes().first(),
            Some(b'a'..=b'z') | Some(b'0'..=b'9')
        )
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b':')
        })
    {
        return Err(format!("schedule status: {label} is not a bounded code").into());
    }
    Ok(())
}

fn bounded_text(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 4096
        || value.chars().any(char::is_control)
    {
        return Err(format!("schedule status: {label} is not bounded text").into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum StatusSourceKindV1 {
    Authority,
    Ledger,
    Evidence,
    Retention,
    Controls,
    Windows,
    Outbox,
    Notifications,
    Ownership,
    Semantics,
}

impl StatusSourceKindV1 {
    const ALL: [Self; 10] = [
        Self::Authority,
        Self::Ledger,
        Self::Evidence,
        Self::Retention,
        Self::Controls,
        Self::Windows,
        Self::Outbox,
        Self::Notifications,
        Self::Ownership,
        Self::Semantics,
    ];

    fn wire(self) -> &'static str {
        match self {
            Self::Authority => "authority",
            Self::Ledger => "ledger",
            Self::Evidence => "evidence",
            Self::Retention => "retention",
            Self::Controls => "controls",
            Self::Windows => "windows",
            Self::Outbox => "outbox",
            Self::Notifications => "notifications",
            Self::Ownership => "ownership",
            Self::Semantics => "semantics",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum StatusSourceStateV1 {
    Healthy { sha256: String },
    Missing { code: String },
    Corrupt { code: String },
    Blocked { code: String },
}

impl StatusSourceStateV1 {
    fn validate(&self) -> Result<(), BoxError> {
        match self {
            Self::Healthy { sha256 } if local_file::valid_sha256(sha256) => Ok(()),
            Self::Healthy { .. } => Err("schedule status: healthy source hash is invalid".into()),
            Self::Missing { code } | Self::Corrupt { code } | Self::Blocked { code } => {
                bounded_code("source disposition", code)
            }
        }
    }

    fn degradation_code(&self) -> Option<&str> {
        match self {
            Self::Healthy { .. } => None,
            Self::Missing { code } | Self::Corrupt { code } | Self::Blocked { code } => Some(code),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusSourceObservationV1 {
    pub(super) source: StatusSourceKindV1,
    pub(super) state: StatusSourceStateV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusDegradationV1 {
    pub(super) code: String,
    pub(super) subject: String,
    pub(super) fingerprint_sha256: String,
}

fn degradation(code: &str, subject: &str) -> Result<StatusDegradationV1, BoxError> {
    bounded_code("degradation code", code)?;
    bounded_text("degradation subject", subject)?;
    let mut bytes = b"a2a-bridge:r3d3:status-degradation:v1\0".to_vec();
    bytes.extend_from_slice(&serde_json::to_vec(&(code, subject))?);
    Ok(StatusDegradationV1 {
        code: code.into(),
        subject: subject.into(),
        fingerprint_sha256: local_file::sha256_hex(&bytes),
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ScheduleOverallStateV1 {
    Green,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectedScheduleStatusV1 {
    pub(super) schema_version: u16,
    pub(super) overall: ScheduleOverallStateV1,
    pub(super) status: ScheduleStatusV1,
    pub(super) sources: Vec<StatusSourceObservationV1>,
    pub(super) degradations: Vec<StatusDegradationV1>,
}

fn last_outcome_text(value: &OptionalTextV1) -> Option<&str> {
    match value {
        OptionalTextV1::Absent => None,
        OptionalTextV1::Text { value } => Some(value),
    }
}

fn record_ref_id(value: &OptionalRecordRefV1) -> Option<&str> {
    match value {
        OptionalRecordRefV1::Absent => None,
        OptionalRecordRefV1::Record { id, .. } => Some(id),
    }
}

fn derive_degradations(
    status: &ScheduleStatusV1,
    sources: &[StatusSourceObservationV1],
) -> Result<Vec<StatusDegradationV1>, BoxError> {
    let mut degradations = Vec::new();
    for source in sources {
        if let Some(code) = source.state.degradation_code() {
            degradations.push(degradation(code, source.source.wire())?);
        }
    }

    if let OptionalAuthorityStatusV1::Authority { state, id, .. } = &status.provider_grant {
        if *state != AuthorityStateV1::Active {
            degradations.push(degradation("provider_authority_not_active", id)?);
        }
    }
    if status.missed_ticks > 0 {
        degradations.push(degradation(
            "missed_tick",
            &format!("count-{}", status.missed_ticks),
        )?);
    }
    match status.storage_state {
        StorageStateV1::Blocked => {
            degradations.push(degradation("storage_blocked", "owner-storage")?)
        }
        StorageStateV1::QuotaPressure => {
            degradations.push(degradation("storage_quota_pressure", "owner-storage")?)
        }
        _ => {}
    }
    match status.fresh_one_shot_compatibility {
        OneShotCompatibilityStateV1::Pass => {}
        OneShotCompatibilityStateV1::Fail => {
            degradations.push(degradation("one_shot_failed", "compatibility")?)
        }
        OneShotCompatibilityStateV1::Unknown => {
            degradations.push(degradation("candidate_unknown", "compatibility")?)
        }
    }
    for case in &status.cases {
        if let Some(hold_id) = record_ref_id(&case.hold) {
            degradations.push(degradation("safety_hold", hold_id)?);
        }
        if let Some(quarantine_id) = record_ref_id(&case.quarantine) {
            degradations.push(degradation("operator_quarantine", quarantine_id)?);
        }
        match case.lifecycle {
            ScheduleCaseLifecycleV1::CharacterizationRequired => {
                degradations.push(degradation("characterization_required", &case.case_id)?)
            }
            ScheduleCaseLifecycleV1::CharacterizedKnownIssue => {
                degradations.push(degradation("known_issue", &case.case_id)?)
            }
            ScheduleCaseLifecycleV1::CharacterizationInconclusive => {
                degradations.push(degradation("characterization_inconclusive", &case.case_id)?)
            }
            _ => {}
        }
        if last_outcome_text(&case.last_outcome)
            .is_some_and(|value| value.contains("candidate_unknown"))
        {
            degradations.push(degradation("candidate_unknown", &case.case_id)?);
        }
    }
    degradations.sort();
    degradations.dedup();
    Ok(degradations)
}

fn validate_sources(sources: &[StatusSourceObservationV1]) -> Result<(), BoxError> {
    if sources.len() != StatusSourceKindV1::ALL.len() {
        return Err("schedule status: every required source must be explicit".into());
    }
    let mut prior = None;
    for source in sources {
        source.state.validate()?;
        if prior.is_some_and(|previous| source.source <= previous) {
            return Err("schedule status: sources are not unique canonical order".into());
        }
        prior = Some(source.source);
    }
    if sources
        .iter()
        .map(|source| source.source)
        .ne(StatusSourceKindV1::ALL)
    {
        return Err("schedule status: required source set is incomplete".into());
    }
    Ok(())
}

impl ProjectedScheduleStatusV1 {
    pub(super) fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 {
            return Err("schedule status: projection version must be 1".into());
        }
        self.status.validate()?;
        validate_sources(&self.sources)?;
        let expected = derive_degradations(&self.status, &self.sources)?;
        if self.degradations != expected
            || (self.overall == ScheduleOverallStateV1::Green) != self.degradations.is_empty()
        {
            return Err("schedule status: overall state omitted or changed a degradation".into());
        }
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn sha256(&self) -> Result<String, BoxError> {
        self.validate()?;
        Ok(local_file::sha256_hex(&canonical_bytes(self)?))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn project_status_from_verified_sources(
    status: ScheduleStatusV1,
    mut sources: Vec<StatusSourceObservationV1>,
) -> Result<ProjectedScheduleStatusV1, BoxError> {
    status.validate()?;
    sources.sort_by_key(|source| source.source);
    validate_sources(&sources)?;
    let degradations = derive_degradations(&status, &sources)?;
    let projected = ProjectedScheduleStatusV1 {
        schema_version: 1,
        overall: if degradations.is_empty() {
            ScheduleOverallStateV1::Green
        } else {
            ScheduleOverallStateV1::Degraded
        },
        status,
        sources,
        degradations,
    };
    projected.validate()?;
    Ok(projected)
}

#[cfg_attr(not(test), allow(dead_code))]
fn healthy_source(source: StatusSourceKindV1, sha256: String) -> StatusSourceObservationV1 {
    StatusSourceObservationV1 {
        source,
        state: StatusSourceStateV1::Healthy { sha256 },
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn missing_source(source: StatusSourceKindV1, code: &str) -> StatusSourceObservationV1 {
    StatusSourceObservationV1 {
        source,
        state: StatusSourceStateV1::Missing { code: code.into() },
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn corrupt_source(source: StatusSourceKindV1, code: &str) -> StatusSourceObservationV1 {
    StatusSourceObservationV1 {
        source,
        state: StatusSourceStateV1::Corrupt { code: code.into() },
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn blocked_source(source: StatusSourceKindV1, code: &str) -> StatusSourceObservationV1 {
    StatusSourceObservationV1 {
        source,
        state: StatusSourceStateV1::Blocked { code: code.into() },
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn domain_source_sha256(domain: &[u8], source_sha256: &str) -> String {
    let mut material = domain.to_vec();
    material.extend_from_slice(source_sha256.as_bytes());
    local_file::sha256_hex(&material)
}

#[cfg_attr(not(test), allow(dead_code))]
fn optional_source(
    source: StatusSourceKindV1,
    acquired: Result<Option<String>, BoxError>,
    missing_code: &str,
    corrupt_code: &str,
) -> StatusSourceObservationV1 {
    match acquired {
        Ok(Some(sha256)) => healthy_source(source, sha256),
        Ok(None) => missing_source(source, missing_code),
        Err(_) => corrupt_source(source, corrupt_code),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn required_source(
    source: StatusSourceKindV1,
    acquired: Result<String, BoxError>,
    corrupt_code: &str,
) -> StatusSourceObservationV1 {
    match acquired {
        Ok(sha256) => healthy_source(source, sha256),
        Err(_) => corrupt_source(source, corrupt_code),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn project_status_from_journals<C>(
    capability: &C,
    status: ScheduleStatusV1,
) -> Result<ProjectedScheduleStatusV1, BoxError>
where
    C: AuthorityStateCapability + AdmissionStateCapability + EvidenceStateCapability + ?Sized,
{
    let authority = optional_source(
        StatusSourceKindV1::Authority,
        authority_status_source_sha256(capability),
        "authority_state_missing",
        "authority_state_corrupt",
    );
    let ledger = required_source(
        StatusSourceKindV1::Ledger,
        ledger_status_source_sha256(capability),
        "ledger_state_corrupt",
    );
    let evidence_result = capability
        .state_quota()
        .reserve(0)
        .map_err(|error| -> BoxError { error })
        .and_then(|_| evidence_status_source_sha256(capability));
    let (evidence, retention) = match evidence_result {
        Ok(Some(sha256)) => (
            healthy_source(StatusSourceKindV1::Evidence, sha256.clone()),
            healthy_source(
                StatusSourceKindV1::Retention,
                domain_source_sha256(b"a2a-bridge:r3d3:status-source:retention:v1\0", &sha256),
            ),
        ),
        Ok(None) => (
            missing_source(StatusSourceKindV1::Evidence, "evidence_state_missing"),
            missing_source(StatusSourceKindV1::Retention, "retention_state_missing"),
        ),
        Err(_) => (
            corrupt_source(StatusSourceKindV1::Evidence, "evidence_state_corrupt"),
            corrupt_source(StatusSourceKindV1::Retention, "retention_state_corrupt"),
        ),
    };
    let (controls, windows) = match admission_status_source_sha256(capability) {
        Ok((controls, windows)) => (
            healthy_source(StatusSourceKindV1::Controls, controls),
            healthy_source(StatusSourceKindV1::Windows, windows),
        ),
        Err(_) => (
            corrupt_source(StatusSourceKindV1::Controls, "controls_state_corrupt"),
            corrupt_source(StatusSourceKindV1::Windows, "windows_state_corrupt"),
        ),
    };
    let outbox = required_source(
        StatusSourceKindV1::Outbox,
        outbox_status_source_sha256(capability),
        "outbox_state_corrupt",
    );
    let notifications = required_source(
        StatusSourceKindV1::Notifications,
        notification_status_source_sha256(capability),
        "notification_state_corrupt",
    );
    let ownership = required_source(
        StatusSourceKindV1::Ownership,
        ownership_status_source_sha256(capability.supervisor_directory()),
        "ownership_state_corrupt",
    );
    // R3d3 has no authoritative scheduler-policy/window projection owner. Hashes prove that the
    // retained journals were acquired, not that a caller-supplied summary agrees with them. Keep
    // this raw acquisition path visibly degraded. R3d5 may add the sole production constructor
    // for `VerifiedScheduleStatusV1` after it derives every semantic field from owned state.
    let semantics = blocked_source(StatusSourceKindV1::Semantics, "status_semantics_unverified");
    project_status_from_verified_sources(
        status,
        vec![
            authority,
            ledger,
            evidence,
            retention,
            controls,
            windows,
            outbox,
            notifications,
            ownership,
            semantics,
        ],
    )
}

#[cfg(test)]
fn project_status(
    status: ScheduleStatusV1,
    sources: Vec<StatusSourceObservationV1>,
) -> Result<ProjectedScheduleStatusV1, BoxError> {
    project_status_from_verified_sources(status, sources)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StatusJournalRecordV1 {
    schema_version: u16,
    generation: u64,
    previous_record: OptionalSha256V1,
    recorded_at_ms: i64,
    projection: ProjectedScheduleStatusV1,
}

/// Closed proof that every semantic field in a status projection was derived from authoritative
/// owner state. R3d3 deliberately supplies no production constructor: R3d5 owns the scheduler
/// policy/window lifecycle needed to create one. This keeps the durable append mechanism present
/// without accepting a raw caller-built `ScheduleStatusV1`.
#[allow(dead_code)]
pub(super) struct VerifiedScheduleStatusV1<'lock> {
    projection: ProjectedScheduleStatusV1,
    _owner_lock: std::marker::PhantomData<&'lock ()>,
}

impl<'lock> VerifiedScheduleStatusV1<'lock> {
    #[cfg(test)]
    fn from_projection_for_test<C: EvidenceStateCapability + ?Sized>(
        _capability: &'lock C,
        projection: ProjectedScheduleStatusV1,
    ) -> Result<Self, BoxError> {
        projection.validate()?;
        if !projection.sources.iter().any(|source| {
            source.source == StatusSourceKindV1::Semantics
                && matches!(source.state, StatusSourceStateV1::Healthy { .. })
        }) {
            return Err("schedule status: semantic projection is not verified".into());
        }
        Ok(Self {
            projection,
            _owner_lock: std::marker::PhantomData,
        })
    }
}

#[allow(dead_code)] // The R3d5 scheduler will append projections; the R3d3 CLI is read-only.
pub(super) struct ScheduleStatusJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    state_quota: Option<StateQuota>,
    records: Vec<(StatusJournalRecordV1, String)>,
}

#[allow(dead_code)] // The R3d5 scheduler will append projections; the R3d3 CLI is read-only.
impl<'lock> ScheduleStatusJournal<'lock> {
    fn generation_name(generation: u64) -> String {
        format!("{STATUS_PREFIX}{generation:020}.json")
    }

    fn entries(directory: &local_file::PinnedDirectory) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule status: retained directory changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "schedule status: non-UTF8 journal entry")?;
            if !name.starts_with(STATUS_PREFIX) {
                continue;
            }
            let raw = name
                .strip_prefix(STATUS_PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
                .ok_or("schedule status: malformed status generation name")?;
            if raw.len() != 20 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("schedule status: malformed status generation".into());
            }
            entries.push((raw.parse::<u64>()?, name));
        }
        if entries.len() > MAX_STATUS_GENERATIONS || !directory.current_path_matches() {
            return Err("schedule status: status journal is unbounded or unstable".into());
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    fn read_record(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<(StatusJournalRecordV1, String), BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let file = directory.open_regular_file(OsStr::new(name), "schedule status record")?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != STATUS_FILE_MODE
            || metadata.len() > MAX_STATUS_RECORD_BYTES
        {
            return Err("schedule status: record is not owner-only mode-0600".into());
        }
        let read = local_file::read_open_regular_file_bounded(
            &file,
            "schedule status record",
            MAX_STATUS_RECORD_BYTES,
        )?;
        let record: StatusJournalRecordV1 = serde_json::from_slice(&read.bytes)
            .map_err(|error| format!("schedule status: invalid record: {error}"))?;
        if canonical_bytes(&record)? != read.bytes {
            return Err("schedule status: record is not canonical JSON".into());
        }
        Ok((record, read.sha256))
    }

    fn open_directory(directory: &'lock local_file::PinnedDirectory) -> Result<Self, BoxError> {
        directory.sync_journal_recovery_barrier("schedule status")?;
        let mut records = Vec::new();
        let mut previous_sha256: Option<String> = None;
        let mut previous_time = None;
        for (index, (generation, name)) in Self::entries(directory)?.into_iter().enumerate() {
            let expected = u64::try_from(index + 1)?;
            if generation != expected {
                return Err("schedule status: status generations are not contiguous".into());
            }
            let (record, sha256) = Self::read_record(directory, &name)?;
            validate_status_record(&record, expected, previous_sha256.as_deref(), previous_time)?;
            previous_sha256 = Some(sha256.clone());
            previous_time = Some(record.recorded_at_ms);
            records.push((record, sha256));
        }
        Ok(Self {
            directory,
            state_quota: None,
            records,
        })
    }

    pub(super) fn open<C: EvidenceStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<Self, BoxError> {
        let mut journal = Self::open_directory(capability.status_directory())?;
        journal.state_quota = Some(capability.state_quota());
        Ok(journal)
    }

    fn append_projected(
        &mut self,
        projection: ProjectedScheduleStatusV1,
    ) -> Result<String, BoxError> {
        projection.validate()?;
        let generation = u64::try_from(self.records.len())?
            .checked_add(1)
            .ok_or("schedule status: generation overflow")?;
        if usize::try_from(generation)? > MAX_STATUS_GENERATIONS {
            return Err("schedule status: generation bound reached".into());
        }
        let previous_sha256 = self.records.last().map(|(_, sha256)| sha256.as_str());
        let previous_time = self.records.last().map(|(record, _)| record.recorded_at_ms);
        let record = StatusJournalRecordV1 {
            schema_version: 1,
            generation,
            previous_record: optional_sha256(previous_sha256),
            recorded_at_ms: projection.status.generated_at_ms,
            projection,
        };
        validate_status_record(&record, generation, previous_sha256, previous_time)?;
        let bytes = canonical_bytes(&record)?;
        let state_quota = self
            .state_quota
            .as_ref()
            .ok_or("schedule status: append requires the owner state quota capability")?;
        self.directory
            .recover_journal_append_residue(STATUS_FILE_MODE, "schedule status record")?;
        state_quota.reserve(bytes.len() as u64)?;
        let name = Self::generation_name(generation);
        self.directory.write_new_journal_record(
            OsStr::new(&name),
            &bytes,
            STATUS_FILE_MODE,
            "schedule status record",
        )?;
        let sha256 = local_file::sha256_hex(&bytes);
        self.records.push((record, sha256.clone()));
        Ok(sha256)
    }

    pub(super) fn append_verified(
        &mut self,
        verified: VerifiedScheduleStatusV1<'lock>,
    ) -> Result<String, BoxError> {
        self.append_projected(verified.projection)
    }

    #[cfg(test)]
    fn append_projected_for_test(
        &mut self,
        projection: ProjectedScheduleStatusV1,
    ) -> Result<String, BoxError> {
        self.append_projected(projection)
    }

    pub(super) fn latest(&self) -> Option<&ProjectedScheduleStatusV1> {
        self.records.last().map(|(record, _)| &record.projection)
    }
}

fn validate_status_record(
    record: &StatusJournalRecordV1,
    expected_generation: u64,
    previous_sha256: Option<&str>,
    previous_time: Option<i64>,
) -> Result<(), BoxError> {
    if record.schema_version != 1
        || record.generation != expected_generation
        || record.recorded_at_ms != record.projection.status.generated_at_ms
        || record.recorded_at_ms <= 0
        || previous_time.is_some_and(|previous| record.recorded_at_ms <= previous)
        || record.previous_record != optional_sha256(previous_sha256)
    {
        return Err("schedule status: record generation, predecessor, or time is invalid".into());
    }
    record.projection.validate()
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum NotificationKindV1 {
    GreenToRed,
    Recovery,
    AuthBlocked,
    CandidateUnknown,
    MissedTick,
    StoragePressure,
    SafetyHold,
    UnreapedOwnership,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusNotificationV1 {
    pub(super) notification_id: String,
    pub(super) fingerprint_sha256: String,
    pub(super) kind: NotificationKindV1,
    pub(super) subject: String,
    pub(super) source_status_sha256: String,
}

#[cfg_attr(not(test), allow(dead_code))]
fn notification(
    kind: NotificationKindV1,
    subject: String,
    source_status_sha256: &str,
) -> Result<StatusNotificationV1, BoxError> {
    bounded_text("notification subject", &subject)?;
    if !local_file::valid_sha256(source_status_sha256) {
        return Err("schedule status: notification source status hash is invalid".into());
    }
    let mut bytes = b"a2a-bridge:r3d3:notification-transition:v1\0".to_vec();
    bytes.extend_from_slice(&serde_json::to_vec(&(
        kind,
        &subject,
        source_status_sha256,
    ))?);
    let fingerprint_sha256 = local_file::sha256_hex(&bytes);
    Ok(StatusNotificationV1 {
        notification_id: format!("notification-{fingerprint_sha256}"),
        fingerprint_sha256,
        kind,
        subject,
        source_status_sha256: source_status_sha256.into(),
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn degradation_fingerprints(value: &ProjectedScheduleStatusV1) -> BTreeSet<&str> {
    value
        .degradations
        .iter()
        .map(|value| value.fingerprint_sha256.as_str())
        .collect()
}

#[cfg_attr(not(test), allow(dead_code))]
fn has_degradation(value: &ProjectedScheduleStatusV1, code: &str, subject: &str) -> bool {
    value
        .degradations
        .iter()
        .any(|value| value.code == code && value.subject == subject)
}

#[cfg_attr(not(test), allow(dead_code))]
fn source_disposition(
    value: &ProjectedScheduleStatusV1,
    source: StatusSourceKindV1,
) -> Option<&str> {
    value
        .sources
        .iter()
        .find(|value| value.source == source)
        .and_then(|value| value.state.degradation_code())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn status_notifications(
    previous: Option<&ProjectedScheduleStatusV1>,
    current: &ProjectedScheduleStatusV1,
) -> Result<Vec<StatusNotificationV1>, BoxError> {
    current.validate()?;
    if let Some(previous) = previous {
        previous.validate()?;
        if current.status.generated_at_ms <= previous.status.generated_at_ms {
            return Err("schedule status: notification transition is not forward in time".into());
        }
    }
    let source_status_sha256 = current.sha256()?;
    let mut notifications = Vec::new();
    let previous_overall = previous.map(|value| value.overall);
    if current.overall == ScheduleOverallStateV1::Degraded
        && previous_overall != Some(ScheduleOverallStateV1::Degraded)
    {
        let degradation_set = degradation_fingerprints(current)
            .into_iter()
            .collect::<Vec<_>>();
        let subject = format!(
            "degraded-{}",
            local_file::sha256_hex(&serde_json::to_vec(&degradation_set)?)
        );
        notifications.push(notification(
            NotificationKindV1::GreenToRed,
            subject,
            &source_status_sha256,
        )?);
    }
    if current.overall == ScheduleOverallStateV1::Green
        && previous_overall == Some(ScheduleOverallStateV1::Degraded)
    {
        notifications.push(notification(
            NotificationKindV1::Recovery,
            "schedule-green".into(),
            &source_status_sha256,
        )?);
    }

    if let OptionalAuthorityStatusV1::Authority { state, id, .. } = &current.status.provider_grant {
        let previously_same = previous
            .is_some_and(|previous| has_degradation(previous, "provider_authority_not_active", id));
        if *state != AuthorityStateV1::Active && !previously_same {
            notifications.push(notification(
                NotificationKindV1::AuthBlocked,
                id.clone(),
                &source_status_sha256,
            )?);
        }
    }
    let previous_missed = previous.map_or(0, |value| value.status.missed_ticks);
    if current.status.missed_ticks > previous_missed {
        notifications.push(notification(
            NotificationKindV1::MissedTick,
            format!("count-{}", current.status.missed_ticks),
            &source_status_sha256,
        )?);
    }
    let current_storage_pressure = matches!(
        current.status.storage_state,
        StorageStateV1::Blocked | StorageStateV1::QuotaPressure
    );
    let previous_storage = previous.map(|value| value.status.storage_state);
    if current_storage_pressure && previous_storage != Some(current.status.storage_state) {
        notifications.push(notification(
            NotificationKindV1::StoragePressure,
            format!("{:?}", current.status.storage_state).to_ascii_lowercase(),
            &source_status_sha256,
        )?);
    }
    for case in &current.status.cases {
        if let Some(hold_id) = record_ref_id(&case.hold) {
            let already = previous.is_some_and(|previous| {
                previous
                    .status
                    .cases
                    .iter()
                    .any(|prior| record_ref_id(&prior.hold) == Some(hold_id))
            });
            if !already {
                notifications.push(notification(
                    NotificationKindV1::SafetyHold,
                    hold_id.into(),
                    &source_status_sha256,
                )?);
            }
        }
        if last_outcome_text(&case.last_outcome)
            .is_some_and(|value| value.contains("candidate_unknown"))
        {
            let already = previous.is_some_and(|previous| {
                previous.status.cases.iter().any(|prior| {
                    prior.case_id == case.case_id
                        && last_outcome_text(&prior.last_outcome)
                            == last_outcome_text(&case.last_outcome)
                })
            });
            if !already {
                notifications.push(notification(
                    NotificationKindV1::CandidateUnknown,
                    case.case_id.clone(),
                    &source_status_sha256,
                )?);
            }
        }
    }
    if current.status.fresh_one_shot_compatibility == OneShotCompatibilityStateV1::Unknown
        && previous.is_none_or(|previous| {
            previous.status.fresh_one_shot_compatibility != OneShotCompatibilityStateV1::Unknown
        })
    {
        notifications.push(notification(
            NotificationKindV1::CandidateUnknown,
            "compatibility".into(),
            &source_status_sha256,
        )?);
    }
    if let Some(code) = source_disposition(current, StatusSourceKindV1::Ownership) {
        let already = previous
            .and_then(|value| source_disposition(value, StatusSourceKindV1::Ownership))
            == Some(code);
        if !already {
            notifications.push(notification(
                NotificationKindV1::UnreapedOwnership,
                code.into(),
                &source_status_sha256,
            )?);
        }
    }
    notifications.sort();
    notifications.dedup_by(|left, right| left.fingerprint_sha256 == right.fingerprint_sha256);
    Ok(notifications)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum NotificationLifecycleV1 {
    Pending,
    Delivered { completed_at_ms: i64 },
    Failed { completed_at_ms: i64, code: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NotificationJournalRecordV1 {
    schema_version: u16,
    generation: u64,
    previous_record: OptionalSha256V1,
    notification: StatusNotificationV1,
    created_at_ms: i64,
    lifecycle: NotificationLifecycleV1,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) trait NotificationSinkV1 {
    fn deliver(&mut self, notification: &StatusNotificationV1) -> Result<(), BoxError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum NotificationFailpointV1 {
    None,
    AfterIntent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct NotificationDeliverySummaryV1 {
    pub(super) delivered: usize,
    pub(super) failed: usize,
    pub(super) deduplicated: usize,
}

#[allow(dead_code)] // The R3d5 scheduler supplies a macOS sink; R3d3 tests inject only fake sinks.
pub(super) struct NotificationJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    state_quota: StateQuota,
    records: Vec<(NotificationJournalRecordV1, String)>,
    latest: BTreeMap<String, NotificationJournalRecordV1>,
}

#[allow(dead_code)] // The R3d5 scheduler supplies a macOS sink; R3d3 tests inject only fake sinks.
impl<'lock> NotificationJournal<'lock> {
    fn generation_name(generation: u64) -> String {
        format!("{NOTIFICATION_PREFIX}{generation:020}.json")
    }

    fn entries(directory: &local_file::PinnedDirectory) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule status: retained notification directory changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "schedule status: non-UTF8 notification entry")?;
            if !name.starts_with(NOTIFICATION_PREFIX) {
                continue;
            }
            let raw = name
                .strip_prefix(NOTIFICATION_PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
                .ok_or("schedule status: malformed notification generation name")?;
            if raw.len() != 20 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("schedule status: malformed notification generation".into());
            }
            entries.push((raw.parse::<u64>()?, name));
        }
        if entries.len() > MAX_STATUS_GENERATIONS || !directory.current_path_matches() {
            return Err("schedule status: notification journal is unbounded or unstable".into());
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    fn read_record(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<(NotificationJournalRecordV1, String), BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let file = directory.open_regular_file(OsStr::new(name), "notification record")?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != STATUS_FILE_MODE
            || metadata.len() > MAX_STATUS_RECORD_BYTES
        {
            return Err("schedule status: notification is not owner-only mode-0600".into());
        }
        let read = local_file::read_open_regular_file_bounded(
            &file,
            "notification record",
            MAX_STATUS_RECORD_BYTES,
        )?;
        let record: NotificationJournalRecordV1 = serde_json::from_slice(&read.bytes)
            .map_err(|error| format!("schedule status: invalid notification: {error}"))?;
        if canonical_bytes(&record)? != read.bytes {
            return Err("schedule status: notification is not canonical JSON".into());
        }
        Ok((record, read.sha256))
    }

    pub(super) fn open<C: EvidenceStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<Self, BoxError> {
        let directory = capability.status_directory();
        directory.sync_journal_recovery_barrier("schedule notification")?;
        let mut records = Vec::new();
        let mut latest = BTreeMap::new();
        let mut previous_sha256: Option<String> = None;
        let mut previous_time = None;
        for (index, (generation, name)) in Self::entries(directory)?.into_iter().enumerate() {
            let expected = u64::try_from(index + 1)?;
            if generation != expected {
                return Err("schedule status: notification generations are not contiguous".into());
            }
            let (record, sha256) = Self::read_record(directory, &name)?;
            validate_notification_record(
                &record,
                expected,
                previous_sha256.as_deref(),
                previous_time,
                latest.get(&record.notification.fingerprint_sha256),
            )?;
            latest.insert(
                record.notification.fingerprint_sha256.clone(),
                record.clone(),
            );
            previous_sha256 = Some(sha256.clone());
            previous_time = Some(notification_event_time(
                &record.lifecycle,
                record.created_at_ms,
            ));
            records.push((record, sha256));
        }
        Ok(Self {
            directory,
            state_quota: capability.state_quota(),
            records,
            latest,
        })
    }

    fn append(
        &mut self,
        notification: StatusNotificationV1,
        created_at_ms: i64,
        lifecycle: NotificationLifecycleV1,
    ) -> Result<(), BoxError> {
        let generation = u64::try_from(self.records.len())?
            .checked_add(1)
            .ok_or("schedule status: notification generation overflow")?;
        validate_notification_generation(generation)?;
        let previous_sha256 = self.records.last().map(|(_, sha256)| sha256.as_str());
        let previous_time = self
            .records
            .last()
            .map(|(record, _)| notification_event_time(&record.lifecycle, record.created_at_ms));
        let record = NotificationJournalRecordV1 {
            schema_version: 1,
            generation,
            previous_record: optional_sha256(previous_sha256),
            notification,
            created_at_ms,
            lifecycle,
        };
        validate_notification_record(
            &record,
            generation,
            previous_sha256,
            previous_time,
            self.latest.get(&record.notification.fingerprint_sha256),
        )?;
        let bytes = canonical_bytes(&record)?;
        self.directory
            .recover_journal_append_residue(STATUS_FILE_MODE, "notification record")?;
        self.state_quota.reserve(bytes.len() as u64)?;
        let name = Self::generation_name(generation);
        self.directory.write_new_journal_record(
            OsStr::new(&name),
            &bytes,
            STATUS_FILE_MODE,
            "notification record",
        )?;
        let sha256 = local_file::sha256_hex(&bytes);
        self.latest.insert(
            record.notification.fingerprint_sha256.clone(),
            record.clone(),
        );
        self.records.push((record, sha256));
        Ok(())
    }

    fn contains(&self, fingerprint_sha256: &str) -> bool {
        self.latest.contains_key(fingerprint_sha256)
    }

    pub(super) fn recover_ambiguous(&mut self, mut now_ms: i64) -> Result<usize, BoxError> {
        let pending = self
            .latest
            .values()
            .filter(|record| record.lifecycle == NotificationLifecycleV1::Pending)
            .map(|record| record.notification.clone())
            .collect::<Vec<_>>();
        for notification in &pending {
            self.append(
                notification.clone(),
                self.latest[&notification.fingerprint_sha256].created_at_ms,
                NotificationLifecycleV1::Failed {
                    completed_at_ms: now_ms,
                    code: "delivery_outcome_unknown".into(),
                },
            )?;
            now_ms = now_ms
                .checked_add(1)
                .ok_or("schedule status: notification recovery time overflow")?;
        }
        Ok(pending.len())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn notification_status_source_sha256<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
) -> Result<String, BoxError> {
    let journal = NotificationJournal::open(capability)?;
    let mut material = b"a2a-bridge:r3d3:status-source:notifications:v1\0".to_vec();
    material.extend_from_slice(
        journal
            .records
            .last()
            .map_or("empty", |(_, sha256)| sha256.as_str())
            .as_bytes(),
    );
    Ok(local_file::sha256_hex(&material))
}

fn notification_event_time(lifecycle: &NotificationLifecycleV1, created_at_ms: i64) -> i64 {
    match lifecycle {
        NotificationLifecycleV1::Pending => created_at_ms,
        NotificationLifecycleV1::Delivered { completed_at_ms }
        | NotificationLifecycleV1::Failed {
            completed_at_ms, ..
        } => *completed_at_ms,
    }
}

fn validate_notification_generation(generation: u64) -> Result<(), BoxError> {
    if generation == 0 || usize::try_from(generation)? > MAX_STATUS_GENERATIONS {
        return Err("schedule status: notification generation bound reached".into());
    }
    Ok(())
}

fn validate_notification_record(
    record: &NotificationJournalRecordV1,
    expected_generation: u64,
    previous_sha256: Option<&str>,
    previous_time: Option<i64>,
    prior_same: Option<&NotificationJournalRecordV1>,
) -> Result<(), BoxError> {
    bounded_text("notification id", &record.notification.notification_id)?;
    bounded_text("notification subject", &record.notification.subject)?;
    if record.schema_version != 1
        || record.generation != expected_generation
        || record.created_at_ms <= 0
        || record.previous_record != optional_sha256(previous_sha256)
        || !local_file::valid_sha256(&record.notification.fingerprint_sha256)
        || !local_file::valid_sha256(&record.notification.source_status_sha256)
        || record.notification.notification_id
            != format!("notification-{}", record.notification.fingerprint_sha256)
    {
        return Err("schedule status: notification identity or predecessor is invalid".into());
    }
    let event_time = notification_event_time(&record.lifecycle, record.created_at_ms);
    if previous_time.is_some_and(|previous| event_time <= previous) {
        return Err("schedule status: notification event is backdated".into());
    }
    match (&record.lifecycle, prior_same) {
        (NotificationLifecycleV1::Pending, None) => {}
        (NotificationLifecycleV1::Pending, Some(_)) => {
            return Err("schedule status: notification fingerprint was duplicated".into())
        }
        (NotificationLifecycleV1::Delivered { completed_at_ms }, Some(prior))
            if prior.lifecycle == NotificationLifecycleV1::Pending
                && prior.notification == record.notification
                && prior.created_at_ms == record.created_at_ms
                && *completed_at_ms > record.created_at_ms => {}
        (
            NotificationLifecycleV1::Failed {
                completed_at_ms,
                code,
            },
            Some(prior),
        ) if prior.lifecycle == NotificationLifecycleV1::Pending
            && prior.notification == record.notification
            && prior.created_at_ms == record.created_at_ms
            && *completed_at_ms > record.created_at_ms =>
        {
            bounded_code("notification failure", code)?;
        }
        _ => return Err("schedule status: notification lifecycle is invalid".into()),
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn deliver_status_notifications<S: NotificationSinkV1>(
    previous: Option<&ProjectedScheduleStatusV1>,
    current: &ProjectedScheduleStatusV1,
    journal: &mut NotificationJournal<'_>,
    sink: &mut S,
    mut now_ms: i64,
    failpoint: NotificationFailpointV1,
) -> Result<NotificationDeliverySummaryV1, BoxError> {
    let mut summary = NotificationDeliverySummaryV1::default();
    for notification in status_notifications(previous, current)? {
        if journal.contains(&notification.fingerprint_sha256) {
            summary.deduplicated += 1;
            continue;
        }
        journal.append(
            notification.clone(),
            now_ms,
            NotificationLifecycleV1::Pending,
        )?;
        if failpoint == NotificationFailpointV1::AfterIntent {
            return Err("schedule status: injected crash after notification intent".into());
        }
        let completed_at_ms = now_ms
            .checked_add(1)
            .ok_or("schedule status: notification completion time overflow")?;
        match sink.deliver(&notification) {
            Ok(()) => {
                journal.append(
                    notification,
                    now_ms,
                    NotificationLifecycleV1::Delivered { completed_at_ms },
                )?;
                summary.delivered += 1;
            }
            Err(_) => {
                journal.append(
                    notification,
                    now_ms,
                    NotificationLifecycleV1::Failed {
                        completed_at_ms,
                        code: "delivery_failed".into(),
                    },
                )?;
                summary.failed += 1;
            }
        }
        now_ms = completed_at_ms
            .checked_add(1)
            .ok_or("schedule status: notification sequence time overflow")?;
    }
    Ok(summary)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PersistedQuarantineOpeningV1 {
    pub(super) source_generation: u64,
    pub(super) source_record_sha256: String,
    pub(super) opening: QuarantineV1,
}

pub(super) trait QuarantineClosureStoreV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn read_active_opening(
        &self,
        profile_sha256: &str,
    ) -> Result<PersistedQuarantineOpeningV1, BoxError>;

    #[cfg_attr(not(test), allow(dead_code))]
    fn append_closure(
        &mut self,
        opening: &PersistedQuarantineOpeningV1,
        closure: &QuarantineV1,
    ) -> Result<(), BoxError>;
}

#[cfg_attr(not(test), allow(dead_code))]
fn quarantine_opening_sha256(opening: &QuarantineV1) -> Result<String, BoxError> {
    opening.validate()?;
    if !matches!(opening, QuarantineV1::Open { .. }) {
        return Err("schedule status: quarantine history is not an opening".into());
    }
    let mut bytes = b"a2a-bridge:r3d2:quarantine-opening:v1\0".to_vec();
    bytes.extend_from_slice(&serde_json::to_vec(opening)?);
    Ok(local_file::sha256_hex(&bytes))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn close_quarantine_dereferenced<S: QuarantineClosureStoreV1>(
    store: &mut S,
    profile_sha256: &str,
    operator: String,
    reason: String,
    closed_at_ms: i64,
) -> Result<QuarantineV1, BoxError> {
    if !local_file::valid_sha256(profile_sha256) {
        return Err("schedule status: persisted quarantine source identity is invalid".into());
    }
    let opening = store.read_active_opening(profile_sha256)?;
    if !local_file::valid_sha256(&opening.source_record_sha256) {
        return Err("schedule status: persisted quarantine source identity is invalid".into());
    }
    let QuarantineV1::Open {
        schema_version,
        quarantine_id,
        profile,
        created_at_ms,
        ..
    } = &opening.opening
    else {
        return Err("schedule status: active quarantine source is not open".into());
    };
    if opening.source_generation == 0
        || profile.sha256 != profile_sha256
        || closed_at_ms <= *created_at_ms
    {
        return Err("schedule status: persisted quarantine opening does not match closure".into());
    }
    let closure = QuarantineV1::Closed {
        schema_version: *schema_version,
        quarantine_id: quarantine_id.clone(),
        profile: profile.clone(),
        opening_sha256: quarantine_opening_sha256(&opening.opening)?,
        operator,
        reason,
        created_at_ms: *created_at_ms,
        closed_at_ms,
    };
    closure.validate()?;
    store.append_closure(&opening, &closure)?;
    Ok(closure)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProductionStatusStateV1 {
    NotInitialized,
    Green,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProductionStatusReportV1 {
    schema_version: u16,
    state: ProductionStatusStateV1,
    activation: String,
    effects: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<ProjectedScheduleStatusV1>,
}

fn status_report(directory: Option<&local_file::PinnedDirectory>) -> ProductionStatusReportV1 {
    let Some(directory) = directory else {
        return ProductionStatusReportV1 {
            schema_version: 1,
            state: ProductionStatusStateV1::NotInitialized,
            activation: "r3d5_activation_not_enabled".into(),
            effects: "no_effects".into(),
            reason: "not_initialized".into(),
            status: None,
        };
    };
    match ScheduleStatusJournal::open_directory(directory) {
        Ok(journal) => match journal.latest().cloned() {
            Some(status) => ProductionStatusReportV1 {
                schema_version: 1,
                state: if status.overall == ScheduleOverallStateV1::Green {
                    ProductionStatusStateV1::Green
                } else {
                    ProductionStatusStateV1::Degraded
                },
                activation: "r3d5_activation_not_enabled".into(),
                effects: "no_effects".into(),
                reason: if status.overall == ScheduleOverallStateV1::Green {
                    "status_available"
                } else {
                    "status_degraded"
                }
                .into(),
                status: Some(status),
            },
            None => ProductionStatusReportV1 {
                schema_version: 1,
                state: ProductionStatusStateV1::NotInitialized,
                activation: "r3d5_activation_not_enabled".into(),
                effects: "no_effects".into(),
                reason: "status_not_initialized".into(),
                status: None,
            },
        },
        Err(_) => ProductionStatusReportV1 {
            schema_version: 1,
            state: ProductionStatusStateV1::Degraded,
            activation: "r3d5_activation_not_enabled".into(),
            effects: "no_effects".into(),
            reason: "status_state_corrupt".into(),
            status: None,
        },
    }
}

fn unavailable_status_report() -> ProductionStatusReportV1 {
    ProductionStatusReportV1 {
        schema_version: 1,
        state: ProductionStatusStateV1::Degraded,
        activation: "r3d5_activation_not_enabled".into(),
        effects: "no_effects".into(),
        reason: "status_state_unavailable".into(),
        status: None,
    }
}

fn render_report(report: &ProductionStatusReportV1, json: bool) -> Result<String, BoxError> {
    if json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let state = match report.state {
        ProductionStatusStateV1::NotInitialized => "not_initialized",
        ProductionStatusStateV1::Green => "green",
        ProductionStatusStateV1::Degraded => "degraded",
    };
    let mut rendered = format!(
        "state: {state}\nactivation: {}\neffects: {}\nreason: {}\n",
        report.activation, report.effects, report.reason
    );
    if let Some(status) = &report.status {
        rendered.push_str(&format!(
            "generated_at_ms: {}\nmissed_ticks: {}\ndegradations: {}\n",
            status.status.generated_at_ms,
            status.status.missed_ticks,
            status.degradations.len()
        ));
    }
    Ok(rendered)
}

pub(super) fn render_production_status(json: bool) -> Result<String, BoxError> {
    let report = match open_production_status_directory_read_only() {
        Ok(directory) => status_report(directory.as_ref()),
        Err(_) => unavailable_status_report(),
    };
    render_report(&report, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_schedule_authority::FileAuthorityJournal;
    use crate::compatibility_schedule_evidence::{EvidenceStateModelV1, FileEvidenceJournal};
    use crate::compatibility_schedule_schema::{
        ColdStorageBindingV1, FingerprintV1, OptionalWindowV1, ScheduleCaseStatusV1,
        SharedOperatorHealthV1,
    };
    use crate::compatibility_schedule_state::SchedulerStateRoot;
    use std::cell::Cell;
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn status(generated_at_ms: i64) -> ScheduleStatusV1 {
        ScheduleStatusV1 {
            schema_version: 1,
            generated_at_ms,
            policy_sha256: "a".repeat(64),
            last_window: OptionalWindowV1::Window {
                id: "window-1".into(),
                scheduled_at_ms: generated_at_ms - 5,
            },
            next_window: OptionalWindowV1::Window {
                id: "window-2".into(),
                scheduled_at_ms: generated_at_ms + 5,
            },
            provider_grant: OptionalAuthorityStatusV1::Authority {
                id: "grant-1".into(),
                sha256: "b".repeat(64),
                state: AuthorityStateV1::Active,
                expires_at_ms: generated_at_ms + 100,
                revocation_generation: 1,
            },
            storage_consent: OptionalAuthorityStatusV1::Absent,
            ledger_headroom_sha256: "c".repeat(64),
            storage_state: StorageStateV1::HotOnly,
            missed_ticks: 0,
            fresh_one_shot_compatibility: OneShotCompatibilityStateV1::Pass,
            shared_operator_health: SharedOperatorHealthV1::NotEvaluated,
            cases: vec![ScheduleCaseStatusV1 {
                case_id: "case-1".into(),
                lifecycle: ScheduleCaseLifecycleV1::ScheduledActive,
                last_outcome: OptionalTextV1::Text {
                    value: "candidate_pass".into(),
                },
                hold: OptionalRecordRefV1::Absent,
                quarantine: OptionalRecordRefV1::Absent,
            }],
        }
    }

    fn tree_snapshot(root: &Path) -> Vec<(PathBuf, u32, Vec<u8>)> {
        fn walk(root: &Path, current: &Path, snapshot: &mut Vec<(PathBuf, u32, Vec<u8>)>) {
            let mut entries = std::fs::read_dir(current)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                let metadata = std::fs::symlink_metadata(&path).unwrap();
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                let mode = metadata.permissions().mode() & 0o777;
                if metadata.is_dir() {
                    snapshot.push((relative, mode, Vec::new()));
                    walk(root, &path, snapshot);
                } else {
                    snapshot.push((relative, mode, std::fs::read(&path).unwrap()));
                }
            }
        }

        let mut snapshot = Vec::new();
        walk(root, root, &mut snapshot);
        snapshot
    }

    #[test]
    fn injected_operator_home_status_reads_are_complete_and_write_free() {
        let operator_home = tempfile::tempdir().unwrap();
        let missing_before = tree_snapshot(operator_home.path());
        assert!(
            crate::compatibility_schedule_state::open_production_status_directory_read_only_at(
                operator_home.path(),
                false,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(tree_snapshot(operator_home.path()), missing_before);

        let production_root = operator_home
            .path()
            .join("Library/Application Support/a2a-bridge/operator/compatibility-scheduler");
        std::fs::create_dir_all(&production_root).unwrap();
        std::fs::set_permissions(&production_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let scheduler = SchedulerStateRoot::initialize_for_test(&production_root).unwrap();
        let owner = scheduler
            .try_owner_admission("test/injected-production-status")
            .unwrap();
        let mut journal = ScheduleStatusJournal::open(&owner).unwrap();
        journal
            .append_projected_for_test(project_status(status(900), healthy_sources()).unwrap())
            .unwrap();
        drop(journal);
        drop(owner);

        let valid_before = tree_snapshot(operator_home.path());
        let status_directory =
            crate::compatibility_schedule_state::open_production_status_directory_read_only_at(
                operator_home.path(),
                false,
            )
            .unwrap()
            .unwrap();
        let valid = status_report(Some(&status_directory));
        assert_eq!(valid.state, ProductionStatusStateV1::Green);
        assert_eq!(valid.reason, "status_available");
        assert!(render_report(&valid, false)
            .unwrap()
            .contains("effects: no_effects"));
        drop(status_directory);
        assert_eq!(tree_snapshot(operator_home.path()), valid_before);

        std::fs::write(
            production_root
                .join("status")
                .join(ScheduleStatusJournal::generation_name(1)),
            b"{}\n",
        )
        .unwrap();
        let corrupt_before = tree_snapshot(operator_home.path());
        let status_directory =
            crate::compatibility_schedule_state::open_production_status_directory_read_only_at(
                operator_home.path(),
                false,
            )
            .unwrap()
            .unwrap();
        let corrupt = status_report(Some(&status_directory));
        assert_eq!(corrupt.state, ProductionStatusStateV1::Degraded);
        assert_eq!(corrupt.reason, "status_state_corrupt");
        drop(status_directory);
        assert_eq!(tree_snapshot(operator_home.path()), corrupt_before);
    }

    fn healthy_sources() -> Vec<StatusSourceObservationV1> {
        StatusSourceKindV1::ALL
            .into_iter()
            .enumerate()
            .map(|(index, source)| StatusSourceObservationV1 {
                source,
                state: StatusSourceStateV1::Healthy {
                    sha256: format!("{:064x}", index + 1),
                },
            })
            .collect()
    }

    #[test]
    fn status_projection_never_turns_missing_corrupt_or_blocking_inputs_green() {
        let green = project_status(status(100), healthy_sources()).unwrap();
        assert_eq!(green.overall, ScheduleOverallStateV1::Green);

        for (index, source) in StatusSourceKindV1::ALL.into_iter().enumerate() {
            let mut sources = healthy_sources();
            sources[index].state = if index % 2 == 0 {
                StatusSourceStateV1::Missing {
                    code: format!("missing-{}", source.wire()),
                }
            } else {
                StatusSourceStateV1::Corrupt {
                    code: format!("corrupt-{}", source.wire()),
                }
            };
            let projected = project_status(status(101 + index as i64), sources).unwrap();
            assert_eq!(projected.overall, ScheduleOverallStateV1::Degraded);
            assert!(projected
                .degradations
                .iter()
                .any(|value| value.subject == source.wire()));
        }

        let mut unknown = status(200);
        unknown.fresh_one_shot_compatibility = OneShotCompatibilityStateV1::Unknown;
        unknown.cases[0].last_outcome = OptionalTextV1::Text {
            value: "candidate_unknown:catalog_unavailable".into(),
        };
        assert_eq!(
            project_status(unknown, healthy_sources()).unwrap().overall,
            ScheduleOverallStateV1::Degraded
        );
    }

    #[test]
    fn journal_acquisition_cannot_project_missing_or_corrupt_required_state_green() {
        let directory = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let combined = scheduler
            .try_owner_admission("status-source-acquisition")
            .unwrap()
            .try_authority_state("status-source-authority")
            .unwrap();

        let missing = project_status_from_journals(&combined, status(210)).unwrap();
        assert_eq!(missing.overall, ScheduleOverallStateV1::Degraded);
        assert!(matches!(
            missing.sources[0].state,
            StatusSourceStateV1::Missing { ref code } if code == "authority_state_missing"
        ));
        assert!(matches!(
            missing.sources[2].state,
            StatusSourceStateV1::Missing { ref code } if code == "evidence_state_missing"
        ));

        FileAuthorityJournal::initialize(&combined, 1).unwrap();
        let evidence =
            EvidenceStateModelV1::new("d".repeat(64), ColdStorageBindingV1::Absent).unwrap();
        FileEvidenceJournal::initialize(&combined, &evidence, 2).unwrap();
        let unverified = project_status_from_journals(&combined, status(211)).unwrap();
        assert_eq!(unverified.overall, ScheduleOverallStateV1::Degraded);
        assert!(unverified.sources.iter().any(|source| matches!(
            source.state,
            StatusSourceStateV1::Blocked { ref code }
                if code == "status_semantics_unverified"
        )));
        assert!(VerifiedScheduleStatusV1::from_projection_for_test(&combined, unverified).is_err());

        let evidence_path = combined
            .evidence_index_directory()
            .canonical_path()
            .join("evidence-state.00000000000000000001.json");
        std::fs::write(&evidence_path, b"{}\n").unwrap();
        let corrupt = project_status_from_journals(&combined, status(212)).unwrap();
        assert_eq!(corrupt.overall, ScheduleOverallStateV1::Degraded);
        assert!(matches!(
            corrupt.sources[2].state,
            StatusSourceStateV1::Corrupt { ref code } if code == "evidence_state_corrupt"
        ));
        assert!(matches!(
            corrupt.sources[3].state,
            StatusSourceStateV1::Corrupt { ref code } if code == "retention_state_corrupt"
        ));
    }

    #[test]
    fn status_journal_reopens_contiguous_chain_and_rejects_gap_or_backdating() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root.try_owner_admission("status-journal").unwrap();
        let mut journal = ScheduleStatusJournal::open(&lock).unwrap();
        let first = project_status(status(300), healthy_sources()).unwrap();
        journal
            .append_verified(
                VerifiedScheduleStatusV1::from_projection_for_test(&lock, first.clone()).unwrap(),
            )
            .unwrap();
        let second = project_status(status(301), healthy_sources()).unwrap();
        journal.append_projected_for_test(second.clone()).unwrap();
        assert!(journal
            .append_projected_for_test(project_status(status(301), healthy_sources()).unwrap())
            .is_err());
        let third = project_status(status(302), healthy_sources()).unwrap();
        journal.append_projected_for_test(third.clone()).unwrap();
        drop(journal);
        assert_eq!(
            ScheduleStatusJournal::open(&lock).unwrap().latest(),
            Some(&third)
        );

        let second_path = lock
            .status_directory()
            .canonical_path()
            .join(ScheduleStatusJournal::generation_name(2));
        let bytes = std::fs::read(&second_path).unwrap();
        std::fs::remove_file(&second_path).unwrap();
        assert!(ScheduleStatusJournal::open(&lock).is_err());
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut restored = options.open(&second_path).unwrap();
        restored.write_all(&bytes).unwrap();
        restored.sync_all().unwrap();
        lock.status_directory().sync().unwrap();
        assert!(ScheduleStatusJournal::open(&lock).is_ok());
    }

    #[test]
    fn status_interruption_before_atomic_publish_reopens_and_retries_same_generation() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root
            .try_owner_admission("status-interrupted-publication")
            .unwrap();
        let status_directory = lock.status_directory();
        let temporary = status_directory
            .canonical_path()
            .join(".a2a-journal-append-v1.tmp");
        let final_record = status_directory
            .canonical_path()
            .join(ScheduleStatusJournal::generation_name(1));
        let projection = project_status(status(350), healthy_sources()).unwrap();
        let mut journal = ScheduleStatusJournal::open(&lock).unwrap();

        status_directory.fail_journal_publish_on_nth_call_for_test(1);
        let error = journal
            .append_projected_for_test(projection.clone())
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("injected failure before journal publication"));
        assert!(temporary.is_file());
        assert!(!final_record.exists());
        drop(journal);

        let mut reopened = ScheduleStatusJournal::open(&lock).unwrap();
        assert!(reopened.latest().is_none());
        assert!(NotificationJournal::open(&lock).is_ok());
        reopened.append_projected_for_test(projection).unwrap();
        assert!(!temporary.exists());
        assert!(final_record.is_file());
        assert!(ScheduleStatusJournal::open(&lock)
            .unwrap()
            .latest()
            .is_some());
    }

    #[test]
    fn status_ambiguous_directory_sync_requires_recovery_barrier_before_reopen() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root
            .try_owner_admission("status-ambiguous-directory-sync")
            .unwrap();
        let status_directory = lock.status_directory();
        let final_record = status_directory
            .canonical_path()
            .join(ScheduleStatusJournal::generation_name(1));
        let projection = project_status(status(355), healthy_sources()).unwrap();
        let mut journal = ScheduleStatusJournal::open(&lock).unwrap();

        status_directory.fail_sync_on_nth_call_for_test(1);
        let error = journal.append_projected_for_test(projection).unwrap_err();
        assert!(error
            .to_string()
            .contains("publication renamed but directory sync is ambiguous"));
        assert!(final_record.is_file());
        drop(journal);

        status_directory.fail_sync_on_nth_call_for_test(1);
        let recovery_error = ScheduleStatusJournal::open(&lock)
            .err()
            .expect("reopen must fail closed until the visible status record is durable");
        assert!(recovery_error
            .to_string()
            .contains("journal recovery barrier"));
        assert!(ScheduleStatusJournal::open(&lock)
            .unwrap()
            .latest()
            .is_some());
    }

    #[test]
    fn notification_generation_bound_rejects_the_first_unreopenable_generation() {
        let first_unreopenable = u64::try_from(MAX_STATUS_GENERATIONS).unwrap() + 1;
        assert!(validate_notification_generation(first_unreopenable).is_err());
        assert!(validate_notification_generation(first_unreopenable - 1).is_ok());
    }

    #[test]
    fn notification_interruption_before_atomic_publish_reopens_and_retries_same_generation() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root
            .try_owner_admission("notification-interrupted-publication")
            .unwrap();
        let status_directory = lock.status_directory();
        let temporary = status_directory
            .canonical_path()
            .join(".a2a-journal-append-v1.tmp");
        let final_record = status_directory
            .canonical_path()
            .join(NotificationJournal::generation_name(1));
        let green = project_status(status(360), healthy_sources()).unwrap();
        let mut unknown_status = status(361);
        unknown_status.fresh_one_shot_compatibility = OneShotCompatibilityStateV1::Unknown;
        unknown_status.cases[0].last_outcome = OptionalTextV1::Text {
            value: "candidate_unknown:catalog_unavailable".into(),
        };
        let unknown = project_status(unknown_status, healthy_sources()).unwrap();
        let notification = status_notifications(Some(&green), &unknown)
            .unwrap()
            .into_iter()
            .next()
            .expect("green-to-unknown transition must notify");
        let mut journal = NotificationJournal::open(&lock).unwrap();

        status_directory.fail_journal_publish_on_nth_call_for_test(1);
        let error = journal
            .append(notification.clone(), 500, NotificationLifecycleV1::Pending)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("injected failure before journal publication"));
        assert!(temporary.is_file());
        assert!(!final_record.exists());
        drop(journal);

        let mut reopened = NotificationJournal::open(&lock).unwrap();
        assert!(reopened.records.is_empty());
        assert!(ScheduleStatusJournal::open(&lock).is_ok());
        reopened
            .append(notification, 500, NotificationLifecycleV1::Pending)
            .unwrap();
        assert!(!temporary.exists());
        assert!(final_record.is_file());
        assert_eq!(NotificationJournal::open(&lock).unwrap().records.len(), 1);
    }

    #[test]
    fn notification_ambiguous_directory_sync_requires_recovery_barrier_before_reopen() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root
            .try_owner_admission("notification-ambiguous-directory-sync")
            .unwrap();
        let status_directory = lock.status_directory();
        let final_record = status_directory
            .canonical_path()
            .join(NotificationJournal::generation_name(1));
        let green = project_status(status(365), healthy_sources()).unwrap();
        let mut unknown_status = status(366);
        unknown_status.fresh_one_shot_compatibility = OneShotCompatibilityStateV1::Unknown;
        unknown_status.cases[0].last_outcome = OptionalTextV1::Text {
            value: "candidate_unknown:catalog_unavailable".into(),
        };
        let unknown = project_status(unknown_status, healthy_sources()).unwrap();
        let notification = status_notifications(Some(&green), &unknown)
            .unwrap()
            .into_iter()
            .next()
            .expect("green-to-unknown transition must notify");
        let mut journal = NotificationJournal::open(&lock).unwrap();

        status_directory.fail_sync_on_nth_call_for_test(1);
        let error = journal
            .append(notification, 500, NotificationLifecycleV1::Pending)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("publication renamed but directory sync is ambiguous"));
        assert!(final_record.is_file());
        drop(journal);

        status_directory.fail_sync_on_nth_call_for_test(1);
        let recovery_error = NotificationJournal::open(&lock)
            .err()
            .expect("reopen must fail closed until the visible notification is durable");
        assert!(recovery_error
            .to_string()
            .contains("journal recovery barrier"));
        assert_eq!(NotificationJournal::open(&lock).unwrap().records.len(), 1);
    }

    #[derive(Default)]
    struct FakeSink {
        calls: Vec<String>,
        fail: bool,
    }

    impl NotificationSinkV1 for FakeSink {
        fn deliver(&mut self, notification: &StatusNotificationV1) -> Result<(), BoxError> {
            self.calls.push(notification.fingerprint_sha256.clone());
            if self.fail {
                Err("fake delivery failure".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn notification_transitions_deduplicate_unknown_and_failure_does_not_rewrite_status() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root.try_owner_admission("notification-test").unwrap();
        let green = project_status(status(400), healthy_sources()).unwrap();
        let mut unknown_status = status(401);
        unknown_status.fresh_one_shot_compatibility = OneShotCompatibilityStateV1::Unknown;
        unknown_status.cases[0].last_outcome = OptionalTextV1::Text {
            value: "candidate_unknown:catalog_unavailable".into(),
        };
        let unknown = project_status(unknown_status, healthy_sources()).unwrap();
        let status_before = unknown.clone();
        let mut journal = NotificationJournal::open(&lock).unwrap();
        let mut sink = FakeSink {
            fail: true,
            ..FakeSink::default()
        };
        let first = deliver_status_notifications(
            Some(&green),
            &unknown,
            &mut journal,
            &mut sink,
            500,
            NotificationFailpointV1::None,
        )
        .unwrap();
        assert!(first.failed >= 1);
        assert_eq!(
            unknown, status_before,
            "delivery failure rewrote canary status"
        );
        let calls = sink.calls.len();
        let repeated = deliver_status_notifications(
            Some(&green),
            &unknown,
            &mut journal,
            &mut sink,
            600,
            NotificationFailpointV1::None,
        )
        .unwrap();
        assert_eq!(sink.calls.len(), calls);
        assert!(repeated.deduplicated >= 1);

        let recovered = project_status(status(402), healthy_sources()).unwrap();
        let recovery = status_notifications(Some(&unknown), &recovered).unwrap();
        assert!(recovery
            .iter()
            .any(|value| value.kind == NotificationKindV1::Recovery));

        sink.fail = false;
        let recovery_delivery = deliver_status_notifications(
            Some(&unknown),
            &recovered,
            &mut journal,
            &mut sink,
            700,
            NotificationFailpointV1::None,
        )
        .unwrap();
        assert_eq!(recovery_delivery.delivered, 1);
        let calls_after_recovery = sink.calls.len();

        let mut recurrent_status = status(403);
        recurrent_status.fresh_one_shot_compatibility = OneShotCompatibilityStateV1::Unknown;
        recurrent_status.cases[0].last_outcome = OptionalTextV1::Text {
            value: "candidate_unknown:catalog_unavailable".into(),
        };
        let recurrent = project_status(recurrent_status, healthy_sources()).unwrap();
        let recurrent_delivery = deliver_status_notifications(
            Some(&recovered),
            &recurrent,
            &mut journal,
            &mut sink,
            800,
            NotificationFailpointV1::None,
        )
        .unwrap();
        assert!(recurrent_delivery.delivered >= 1);
        assert!(sink.calls.len() > calls_after_recovery);
    }

    #[test]
    fn ambiguous_notification_intent_is_terminalized_without_a_second_sink_call() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root.try_owner_admission("notification-crash").unwrap();
        let green = project_status(status(700), healthy_sources()).unwrap();
        let mut blocked_sources = healthy_sources();
        blocked_sources[8].state = StatusSourceStateV1::Blocked {
            code: "unreaped-process".into(),
        };
        let blocked = project_status(status(701), blocked_sources).unwrap();
        let mut journal = NotificationJournal::open(&lock).unwrap();
        let mut sink = FakeSink::default();
        assert!(deliver_status_notifications(
            Some(&green),
            &blocked,
            &mut journal,
            &mut sink,
            800,
            NotificationFailpointV1::AfterIntent,
        )
        .is_err());
        assert!(sink.calls.is_empty());
        drop(journal);

        let mut reopened = NotificationJournal::open(&lock).unwrap();
        assert_eq!(reopened.recover_ambiguous(900).unwrap(), 1);
        assert!(sink.calls.is_empty());
    }

    #[derive(Clone)]
    struct FakeQuarantineStore {
        persisted: PersistedQuarantineOpeningV1,
        replacement: Option<PersistedQuarantineOpeningV1>,
        reads: Cell<usize>,
        closure: Option<QuarantineV1>,
    }

    impl QuarantineClosureStoreV1 for FakeQuarantineStore {
        fn read_active_opening(
            &self,
            _profile_sha256: &str,
        ) -> Result<PersistedQuarantineOpeningV1, BoxError> {
            let reads = self.reads.get();
            self.reads.set(reads + 1);
            if reads > 0 {
                if let Some(replacement) = &self.replacement {
                    return Ok(replacement.clone());
                }
            }
            Ok(self.persisted.clone())
        }

        fn append_closure(
            &mut self,
            opening: &PersistedQuarantineOpeningV1,
            closure: &QuarantineV1,
        ) -> Result<(), BoxError> {
            let current = self.read_active_opening(match &opening.opening {
                QuarantineV1::Open { profile, .. } => &profile.sha256,
                QuarantineV1::Closed { .. } => "invalid",
            })?;
            if current.source_generation != opening.source_generation
                || current.source_record_sha256 != opening.source_record_sha256
                || current.opening != opening.opening
            {
                return Err("fake immutable opening was replaced".into());
            }
            self.closure = Some(closure.clone());
            Ok(())
        }
    }

    fn quarantine_store() -> FakeQuarantineStore {
        let profile = FingerprintV1 {
            schema_version: 1,
            sha256: "d".repeat(64),
        };
        FakeQuarantineStore {
            persisted: PersistedQuarantineOpeningV1 {
                source_generation: 1,
                source_record_sha256: "e".repeat(64),
                opening: QuarantineV1::Open {
                    schema_version: 1,
                    quarantine_id: "quarantine-1".into(),
                    profile,
                    operator: "operator".into(),
                    reason: "known issue".into(),
                    created_at_ms: 10,
                    expires_at_ms: 100,
                },
            },
            replacement: None,
            reads: Cell::new(0),
            closure: None,
        }
    }

    #[test]
    fn quarantine_close_hashes_persisted_opening_and_rejects_replacement() {
        let mut store = quarantine_store();
        let profile = "d".repeat(64);
        let closure = close_quarantine_dereferenced(
            &mut store,
            &profile,
            "operator".into(),
            "reviewed clear".into(),
            20,
        )
        .unwrap();
        assert_eq!(store.closure, Some(closure));

        let mut replaced = quarantine_store();
        let mut replacement = replaced.persisted.clone();
        if let QuarantineV1::Open { reason, .. } = &mut replacement.opening {
            *reason = "changed bytes".into();
        }
        replacement.source_record_sha256 = "f".repeat(64);
        replaced.replacement = Some(replacement);
        assert!(close_quarantine_dereferenced(
            &mut replaced,
            &profile,
            "operator".into(),
            "reviewed clear".into(),
            20,
        )
        .is_err());
        assert!(replaced.closure.is_none());
    }

    #[test]
    fn absent_status_render_is_explicit_and_has_zero_writes() {
        let parent = tempfile::tempdir().unwrap();
        let before = std::fs::read_dir(parent.path()).unwrap().count();
        let report = status_report(None);
        let human = render_report(&report, false).unwrap();
        let json = render_report(&report, true).unwrap();
        assert!(human.contains("state: not_initialized"));
        assert!(human.contains("activation: r3d5_activation_not_enabled"));
        assert!(human.contains("effects: no_effects"));
        assert!(json.contains("\"state\": \"not_initialized\""));
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), before);
    }

    #[test]
    fn unavailable_status_render_is_typed_degraded_and_has_no_effects() {
        let report = unavailable_status_report();
        let human = render_report(&report, false).unwrap();
        let json = render_report(&report, true).unwrap();
        assert!(human.contains("state: degraded"));
        assert!(human.contains("reason: status_state_unavailable"));
        assert!(human.contains("effects: no_effects"));
        assert!(json.contains("\"state\": \"degraded\""));
        assert!(json.contains("\"reason\": \"status_state_unavailable\""));
    }
}
