//! Crash-consistent local publication-outbox state for the future R3d4 GitHub publisher.
//!
//! This module has no network client. It persists local intent, validates exact observations supplied
//! by a future publisher, and reports the one recovery action that publisher may perform.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write as _;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule_schema::{
    publication_check_run_binding_sha256, publication_outbox_identity_sha256, CheckConclusionV1,
    FingerprintV1, GitObjectIdV1, OptionalCheckConclusionV1, OptionalCheckRunIdV1,
    OptionalSha256V1, OptionalStableIdV1, PublicationOutboxStateV1, PublicationOutboxV1,
    ValidateRecord,
};
use crate::compatibility_schedule_state::EvidenceStateCapability;
use crate::{local_file, BoxError};

const OUTBOX_PREFIX: &str = "publication-outbox.";
const MAX_OUTBOX_RECORD_BYTES: u64 = 1024 * 1024;
const MAX_OUTBOX_GENERATIONS: usize = 100_000;
const OUTBOX_FILE_MODE: u32 = 0o600;

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, BoxError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_OUTBOX_RECORD_BYTES {
        return Err("schedule outbox: record exceeds the byte bound".into());
    }
    Ok(bytes)
}

fn outbox_payload_sha256(value: &PublicationOutboxV1) -> Result<String, BoxError> {
    value.validate()?;
    Ok(local_file::sha256_hex(&canonical_bytes(value)?))
}

fn optional_sha256(value: Option<&str>) -> OptionalSha256V1 {
    match value {
        Some(value) => OptionalSha256V1::Sha256 {
            value: value.to_owned(),
        },
        None => OptionalSha256V1::Absent,
    }
}

fn bounded_text(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 4096
        || value.chars().any(char::is_control)
    {
        return Err(format!("schedule outbox: {label} is not bounded text").into());
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct OutboxRemoteIdentityV1 {
    pub(super) repository: String,
    pub(super) pull_request: u64,
    pub(super) test_merge_oid: GitObjectIdV1,
    pub(super) context: String,
    pub(super) app_id: String,
    pub(super) external_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RemoteCheckPhaseV1 {
    InProgress,
    Completed { conclusion: CheckConclusionV1 },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RemoteCheckObservationV1 {
    pub(super) schema_version: u16,
    pub(super) repository: String,
    pub(super) pull_request: u64,
    pub(super) test_merge_oid: GitObjectIdV1,
    pub(super) context: String,
    pub(super) app_id: String,
    pub(super) external_id: String,
    pub(super) check_run_id: u64,
    pub(super) phase: RemoteCheckPhaseV1,
    pub(super) observed_at_ms: i64,
}

impl RemoteCheckObservationV1 {
    pub(super) fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1
            || self.pull_request == 0
            || self.check_run_id == 0
            || self.observed_at_ms <= 0
        {
            return Err("schedule outbox: remote observation version/id/time is invalid".into());
        }
        for (label, value) in [
            ("remote repository", self.repository.as_str()),
            ("remote context", self.context.as_str()),
            ("remote app id", self.app_id.as_str()),
            ("remote external id", self.external_id.as_str()),
        ] {
            bounded_text(label, value)?;
        }
        Ok(())
    }

    fn validate_for(
        &self,
        outbox: &PublicationOutboxV1,
        required_phase: RemoteCheckPhaseV1,
        recorded_at_ms: i64,
    ) -> Result<(), BoxError> {
        self.validate()?;
        if self.repository != outbox.repository
            || self.pull_request != outbox.pull_request
            || self.test_merge_oid != outbox.test_merge_oid
            || self.context != outbox.context
            || self.app_id != outbox.app_id
            || self.external_id != outbox.external_id
            || self.phase != required_phase
            || self.observed_at_ms > recorded_at_ms
        {
            return Err(
                "schedule outbox: remote observation does not bind the immutable check identity"
                    .into(),
            );
        }
        Ok(())
    }

    fn sha256(&self) -> Result<String, BoxError> {
        self.validate()?;
        let mut bytes = b"a2a-bridge:r3d3:remote-check-observation:v1\0".to_vec();
        bytes.extend_from_slice(&serde_json::to_vec(self)?);
        Ok(local_file::sha256_hex(&bytes))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum OptionalRemoteCheckObservationV1 {
    Absent,
    Observation { value: RemoteCheckObservationV1 },
}

impl OptionalRemoteCheckObservationV1 {
    fn from_option(value: Option<&RemoteCheckObservationV1>) -> Self {
        match value {
            Some(value) => Self::Observation {
                value: value.clone(),
            },
            None => Self::Absent,
        }
    }

    fn as_ref(&self) -> Option<&RemoteCheckObservationV1> {
        match self {
            Self::Absent => None,
            Self::Observation { value } => Some(value),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TerminalPublicationV1 {
    pub(super) terminal_consumption_id: String,
    pub(super) desired_conclusion: CheckConclusionV1,
    pub(super) evidence_set_sha256: String,
    pub(super) final_guard_sha256: String,
}

impl TerminalPublicationV1 {
    fn validate(&self) -> Result<(), BoxError> {
        bounded_text("terminal consumption id", &self.terminal_consumption_id)?;
        if !local_file::valid_sha256(&self.evidence_set_sha256)
            || !local_file::valid_sha256(&self.final_guard_sha256)
        {
            return Err("schedule outbox: terminal publication hashes are invalid".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutboxRecoveryActionV1 {
    ReconcileCreate,
    AwaitTerminalPreparation,
    PublishTerminalUpdate,
    ReconcileTerminalUpdate,
    ConfirmObservedTerminal,
    Complete,
}

pub(super) fn recovery_action(value: &PublicationOutboxV1) -> OutboxRecoveryActionV1 {
    match value.state {
        PublicationOutboxStateV1::CreateIntent | PublicationOutboxStateV1::CreateUnknown => {
            OutboxRecoveryActionV1::ReconcileCreate
        }
        PublicationOutboxStateV1::RemotePending => OutboxRecoveryActionV1::AwaitTerminalPreparation,
        PublicationOutboxStateV1::Prepared => OutboxRecoveryActionV1::PublishTerminalUpdate,
        PublicationOutboxStateV1::UpdateUnknown => OutboxRecoveryActionV1::ReconcileTerminalUpdate,
        PublicationOutboxStateV1::RemotelyObserved => {
            OutboxRecoveryActionV1::ConfirmObservedTerminal
        }
        PublicationOutboxStateV1::Confirmed => OutboxRecoveryActionV1::Complete,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct OutboxJournalRecordV1 {
    schema_version: u16,
    generation: u64,
    previous_journal_record: OptionalSha256V1,
    recorded_at_ms: i64,
    remote_observation: OptionalRemoteCheckObservationV1,
    outbox: PublicationOutboxV1,
}

fn immutable_identity_matches(prior: &PublicationOutboxV1, next: &PublicationOutboxV1) -> bool {
    prior.outbox_id == next.outbox_id
        && prior.immutable_identity == next.immutable_identity
        && prior.repository == next.repository
        && prior.pull_request == next.pull_request
        && prior.test_merge_oid == next.test_merge_oid
        && prior.context == next.context
        && prior.app_id == next.app_id
        && prior.external_id == next.external_id
}

fn terminal_fields_match(prior: &PublicationOutboxV1, next: &PublicationOutboxV1) -> bool {
    prior.terminal_consumption == next.terminal_consumption
        && prior.desired_conclusion == next.desired_conclusion
        && prior.evidence_set == next.evidence_set
        && prior.final_guard == next.final_guard
}

fn check_run_id(value: &PublicationOutboxV1) -> Option<u64> {
    match value.check_run {
        OptionalCheckRunIdV1::Absent => None,
        OptionalCheckRunIdV1::CheckRun { id } => Some(id),
    }
}

fn remote_observation_sha256(value: &PublicationOutboxV1) -> Option<&str> {
    match &value.remote_observation {
        OptionalSha256V1::Absent => None,
        OptionalSha256V1::Sha256 { value } => Some(value),
    }
}

fn legal_transition(prior: PublicationOutboxStateV1, next: PublicationOutboxStateV1) -> bool {
    matches!(
        (prior, next),
        (
            PublicationOutboxStateV1::CreateIntent,
            PublicationOutboxStateV1::CreateUnknown | PublicationOutboxStateV1::RemotePending
        ) | (
            PublicationOutboxStateV1::CreateUnknown,
            PublicationOutboxStateV1::CreateUnknown | PublicationOutboxStateV1::RemotePending
        ) | (
            PublicationOutboxStateV1::RemotePending,
            PublicationOutboxStateV1::Prepared
        ) | (
            PublicationOutboxStateV1::Prepared,
            PublicationOutboxStateV1::UpdateUnknown | PublicationOutboxStateV1::RemotelyObserved
        ) | (
            PublicationOutboxStateV1::UpdateUnknown,
            PublicationOutboxStateV1::UpdateUnknown | PublicationOutboxStateV1::RemotelyObserved
        ) | (
            PublicationOutboxStateV1::RemotelyObserved,
            PublicationOutboxStateV1::Confirmed
        )
    )
}

fn validate_transition(
    prior: &PublicationOutboxV1,
    next: &PublicationOutboxV1,
    observation: Option<&RemoteCheckObservationV1>,
    recorded_at_ms: i64,
) -> Result<(), BoxError> {
    prior.validate()?;
    next.validate()?;
    if !immutable_identity_matches(prior, next)
        || next.previous_record
            != (OptionalSha256V1::Sha256 {
                value: outbox_payload_sha256(prior)?,
            })
        || !legal_transition(prior.state, next.state)
    {
        return Err(
            "schedule outbox: transition identity, predecessor, or graph is invalid".into(),
        );
    }

    let prior_check_run = check_run_id(prior);
    let next_check_run = check_run_id(next);
    if prior_check_run.is_some() && prior_check_run != next_check_run {
        return Err("schedule outbox: check-run id is write-once".into());
    }
    if prior_check_run.is_none() && next_check_run.is_some() {
        if next.state != PublicationOutboxStateV1::RemotePending {
            return Err(
                "schedule outbox: check-run id appeared outside create reconciliation".into(),
            );
        }
        let observation = observation
            .ok_or("schedule outbox: remote-pending transition lacks an exact observation")?;
        observation.validate_for(prior, RemoteCheckPhaseV1::InProgress, recorded_at_ms)?;
        if next_check_run != Some(observation.check_run_id) {
            return Err("schedule outbox: remote-pending check-run id diverged".into());
        }
    }

    let prior_terminal = matches!(
        prior.state,
        PublicationOutboxStateV1::Prepared
            | PublicationOutboxStateV1::UpdateUnknown
            | PublicationOutboxStateV1::RemotelyObserved
            | PublicationOutboxStateV1::Confirmed
    );
    let next_terminal = matches!(
        next.state,
        PublicationOutboxStateV1::Prepared
            | PublicationOutboxStateV1::UpdateUnknown
            | PublicationOutboxStateV1::RemotelyObserved
            | PublicationOutboxStateV1::Confirmed
    );
    if prior_terminal && !terminal_fields_match(prior, next) {
        return Err("schedule outbox: terminal identity fields are write-once".into());
    }
    if !prior_terminal && next_terminal && next.state != PublicationOutboxStateV1::Prepared {
        return Err("schedule outbox: terminal fields bypassed prepared".into());
    }

    if next.state == PublicationOutboxStateV1::RemotelyObserved {
        let observation = observation
            .ok_or("schedule outbox: terminal observation is required before local confirmation")?;
        let desired = match next.desired_conclusion {
            OptionalCheckConclusionV1::Conclusion { value } => value,
            OptionalCheckConclusionV1::Absent => {
                return Err("schedule outbox: terminal conclusion is absent".into())
            }
        };
        observation.validate_for(
            next,
            RemoteCheckPhaseV1::Completed {
                conclusion: desired,
            },
            recorded_at_ms,
        )?;
        let observation_sha256 = observation.sha256()?;
        if next_check_run != Some(observation.check_run_id)
            || remote_observation_sha256(next) != Some(observation_sha256.as_str())
        {
            return Err("schedule outbox: terminal remote observation hash diverged".into());
        }
    } else if observation.is_some()
        && !(next.state == PublicationOutboxStateV1::RemotePending && prior_check_run.is_none())
    {
        return Err("schedule outbox: unexpected remote observation for transition".into());
    }
    if prior.state == PublicationOutboxStateV1::RemotelyObserved
        && prior.remote_observation != next.remote_observation
    {
        return Err("schedule outbox: remote observation hash is write-once".into());
    }

    let expected_attempts = match next.state {
        PublicationOutboxStateV1::CreateUnknown
        | PublicationOutboxStateV1::UpdateUnknown
        | PublicationOutboxStateV1::RemotelyObserved => prior
            .remote_observation_attempts
            .checked_add(1)
            .ok_or("schedule outbox: observation attempts overflowed")?,
        PublicationOutboxStateV1::RemotePending if prior_check_run.is_none() => prior
            .remote_observation_attempts
            .checked_add(1)
            .ok_or("schedule outbox: observation attempts overflowed")?,
        _ => prior.remote_observation_attempts,
    };
    if next.remote_observation_attempts != expected_attempts {
        return Err("schedule outbox: observation attempt count is not monotonic".into());
    }
    Ok(())
}

#[allow(dead_code)] // R3d4 will drive the persisted outbox; R3d3 only proves the local mechanism.
pub(super) struct PublicationOutboxJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    records: Vec<(OutboxJournalRecordV1, String)>,
    latest: BTreeMap<String, (PublicationOutboxV1, String)>,
}

#[allow(dead_code)] // R3d4 will drive the persisted outbox; R3d3 only proves the local mechanism.
impl<'lock> PublicationOutboxJournal<'lock> {
    fn generation_name(generation: u64) -> String {
        format!("{OUTBOX_PREFIX}{generation:020}.json")
    }

    fn entries(directory: &local_file::PinnedDirectory) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule outbox: retained directory changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "schedule outbox: non-UTF8 journal entry")?;
            let raw = name
                .strip_prefix(OUTBOX_PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
                .ok_or("schedule outbox: unexpected journal entry")?;
            if raw.len() != 20 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("schedule outbox: malformed journal generation".into());
            }
            entries.push((raw.parse::<u64>()?, name));
        }
        if entries.len() > MAX_OUTBOX_GENERATIONS || !directory.current_path_matches() {
            return Err("schedule outbox: journal scan is unbounded or unstable".into());
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    fn read_record(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<(OutboxJournalRecordV1, String), BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let file = directory.open_regular_file(OsStr::new(name), "publication outbox record")?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != OUTBOX_FILE_MODE
            || metadata.len() > MAX_OUTBOX_RECORD_BYTES
        {
            return Err("schedule outbox: journal record is not owner-only mode-0600".into());
        }
        let read = local_file::read_open_regular_file_bounded(
            &file,
            "publication outbox record",
            MAX_OUTBOX_RECORD_BYTES,
        )?;
        let record: OutboxJournalRecordV1 = serde_json::from_slice(&read.bytes)
            .map_err(|error| format!("schedule outbox: invalid journal record: {error}"))?;
        if canonical_bytes(&record)? != read.bytes {
            return Err("schedule outbox: journal record is not canonical JSON".into());
        }
        Ok((record, read.sha256))
    }

    pub(super) fn open<C: EvidenceStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<Self, BoxError> {
        let directory = capability.publication_outbox_directory();
        let mut records = Vec::new();
        let mut latest = BTreeMap::new();
        let mut previous_journal_sha256: Option<String> = None;
        let mut previous_recorded_at_ms: Option<i64> = None;
        for (index, (generation, name)) in Self::entries(directory)?.into_iter().enumerate() {
            let expected_generation = u64::try_from(index + 1)?;
            if generation != expected_generation {
                return Err("schedule outbox: journal generations are not contiguous".into());
            }
            let (record, sha256) = Self::read_record(directory, &name)?;
            if record.generation != generation {
                return Err("schedule outbox: filename and record generation diverged".into());
            }
            if let Some((prior, _)) = latest.get(&record.outbox.outbox_id) {
                validate_transition(
                    prior,
                    &record.outbox,
                    record.remote_observation.as_ref(),
                    record.recorded_at_ms,
                )?;
            } else if record.remote_observation.as_ref().is_some() {
                return Err("schedule outbox: create intent has a remote observation".into());
            }
            validate_record_shape(
                &record,
                expected_generation,
                previous_journal_sha256.as_deref(),
                previous_recorded_at_ms,
                &latest,
            )?;
            latest.insert(
                record.outbox.outbox_id.clone(),
                (
                    record.outbox.clone(),
                    outbox_payload_sha256(&record.outbox)?,
                ),
            );
            previous_journal_sha256 = Some(sha256.clone());
            previous_recorded_at_ms = Some(record.recorded_at_ms);
            records.push((record, sha256));
        }
        Ok(Self {
            directory,
            records,
            latest,
        })
    }

    fn append_record(
        &mut self,
        outbox: PublicationOutboxV1,
        observation: Option<&RemoteCheckObservationV1>,
        recorded_at_ms: i64,
    ) -> Result<String, BoxError> {
        let generation = u64::try_from(self.records.len())?
            .checked_add(1)
            .ok_or("schedule outbox: journal generation overflow")?;
        if usize::try_from(generation)? > MAX_OUTBOX_GENERATIONS {
            return Err("schedule outbox: journal generation bound reached".into());
        }
        let previous_journal_sha256 = self.records.last().map(|(_, sha256)| sha256.as_str());
        let previous_recorded_at_ms = self.records.last().map(|(record, _)| record.recorded_at_ms);
        let record = OutboxJournalRecordV1 {
            schema_version: 1,
            generation,
            previous_journal_record: optional_sha256(previous_journal_sha256),
            recorded_at_ms,
            remote_observation: OptionalRemoteCheckObservationV1::from_option(observation),
            outbox,
        };
        validate_record_shape(
            &record,
            generation,
            previous_journal_sha256,
            previous_recorded_at_ms,
            &self.latest,
        )?;
        let bytes = canonical_bytes(&record)?;
        let name = Self::generation_name(generation);
        let mut file = self.directory.create_new_file(
            OsStr::new(&name),
            OUTBOX_FILE_MODE,
            "publication outbox record",
        )?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("schedule outbox: cannot persist record: {error}"))?;
        self.directory.sync()?;
        let journal_sha256 = local_file::sha256_hex(&bytes);
        let payload_sha256 = outbox_payload_sha256(&record.outbox)?;
        self.latest.insert(
            record.outbox.outbox_id.clone(),
            (record.outbox.clone(), payload_sha256),
        );
        self.records.push((record, journal_sha256.clone()));
        Ok(journal_sha256)
    }

    pub(super) fn start(
        &mut self,
        identity: OutboxRemoteIdentityV1,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        let mut outbox = PublicationOutboxV1 {
            schema_version: 1,
            outbox_id: "outbox:placeholder".into(),
            immutable_identity: FingerprintV1 {
                schema_version: 1,
                sha256: "0".repeat(64),
            },
            previous_record: OptionalSha256V1::Absent,
            state: PublicationOutboxStateV1::CreateIntent,
            repository: identity.repository,
            pull_request: identity.pull_request,
            test_merge_oid: identity.test_merge_oid,
            context: identity.context,
            app_id: identity.app_id,
            external_id: identity.external_id,
            check_run: OptionalCheckRunIdV1::Absent,
            check_run_binding: OptionalSha256V1::Absent,
            terminal_consumption: OptionalStableIdV1::Absent,
            desired_conclusion: OptionalCheckConclusionV1::Absent,
            evidence_set: OptionalSha256V1::Absent,
            final_guard: OptionalSha256V1::Absent,
            remote_observation: OptionalSha256V1::Absent,
            remote_observation_attempts: 0,
        };
        let identity_sha256 = publication_outbox_identity_sha256(&outbox)?;
        outbox.outbox_id = format!("outbox:{identity_sha256}");
        outbox.immutable_identity.sha256 = identity_sha256;
        outbox.validate()?;
        if self.latest.contains_key(&outbox.outbox_id) {
            return Err("schedule outbox: immutable check identity already exists".into());
        }
        self.append_record(outbox.clone(), None, recorded_at_ms)?;
        Ok(outbox)
    }

    fn current(&self, outbox_id: &str) -> Result<&PublicationOutboxV1, BoxError> {
        self.latest
            .get(outbox_id)
            .map(|(record, _)| record)
            .ok_or_else(|| "schedule outbox: unknown outbox identity".into())
    }

    fn successor(&self, outbox_id: &str) -> Result<PublicationOutboxV1, BoxError> {
        let current = self.current(outbox_id)?;
        let mut next = current.clone();
        next.previous_record = OptionalSha256V1::Sha256 {
            value: outbox_payload_sha256(current)?,
        };
        Ok(next)
    }

    pub(super) fn mark_create_unknown(
        &mut self,
        outbox_id: &str,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        let mut next = self.successor(outbox_id)?;
        next.state = PublicationOutboxStateV1::CreateUnknown;
        next.remote_observation_attempts = next
            .remote_observation_attempts
            .checked_add(1)
            .ok_or("schedule outbox: observation attempts overflowed")?;
        self.append_exact(next, None, recorded_at_ms)
    }

    pub(super) fn bind_remote_pending(
        &mut self,
        outbox_id: &str,
        observation: &RemoteCheckObservationV1,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        let mut next = self.successor(outbox_id)?;
        next.state = PublicationOutboxStateV1::RemotePending;
        next.check_run = OptionalCheckRunIdV1::CheckRun {
            id: observation.check_run_id,
        };
        next.check_run_binding = OptionalSha256V1::Sha256 {
            value: publication_check_run_binding_sha256(
                &next.immutable_identity,
                observation.check_run_id,
            )?,
        };
        next.remote_observation_attempts = next
            .remote_observation_attempts
            .checked_add(1)
            .ok_or("schedule outbox: observation attempts overflowed")?;
        self.append_exact(next, Some(observation), recorded_at_ms)
    }

    pub(super) fn prepare_terminal(
        &mut self,
        outbox_id: &str,
        terminal: TerminalPublicationV1,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        terminal.validate()?;
        let mut next = self.successor(outbox_id)?;
        next.state = PublicationOutboxStateV1::Prepared;
        next.terminal_consumption = OptionalStableIdV1::StableId {
            value: terminal.terminal_consumption_id,
        };
        next.desired_conclusion = OptionalCheckConclusionV1::Conclusion {
            value: terminal.desired_conclusion,
        };
        next.evidence_set = OptionalSha256V1::Sha256 {
            value: terminal.evidence_set_sha256,
        };
        next.final_guard = OptionalSha256V1::Sha256 {
            value: terminal.final_guard_sha256,
        };
        self.append_exact(next, None, recorded_at_ms)
    }

    pub(super) fn mark_update_unknown(
        &mut self,
        outbox_id: &str,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        let mut next = self.successor(outbox_id)?;
        next.state = PublicationOutboxStateV1::UpdateUnknown;
        next.remote_observation_attempts = next
            .remote_observation_attempts
            .checked_add(1)
            .ok_or("schedule outbox: observation attempts overflowed")?;
        self.append_exact(next, None, recorded_at_ms)
    }

    pub(super) fn observe_terminal(
        &mut self,
        outbox_id: &str,
        observation: &RemoteCheckObservationV1,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        let mut next = self.successor(outbox_id)?;
        next.state = PublicationOutboxStateV1::RemotelyObserved;
        next.remote_observation = OptionalSha256V1::Sha256 {
            value: observation.sha256()?,
        };
        next.remote_observation_attempts = next
            .remote_observation_attempts
            .checked_add(1)
            .ok_or("schedule outbox: observation attempts overflowed")?;
        self.append_exact(next, Some(observation), recorded_at_ms)
    }

    pub(super) fn confirm(
        &mut self,
        outbox_id: &str,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        let mut next = self.successor(outbox_id)?;
        next.state = PublicationOutboxStateV1::Confirmed;
        self.append_exact(next, None, recorded_at_ms)
    }

    pub(super) fn append_exact(
        &mut self,
        next: PublicationOutboxV1,
        observation: Option<&RemoteCheckObservationV1>,
        recorded_at_ms: i64,
    ) -> Result<PublicationOutboxV1, BoxError> {
        let prior = self.current(&next.outbox_id)?.clone();
        validate_transition(&prior, &next, observation, recorded_at_ms)?;
        self.append_record(next.clone(), observation, recorded_at_ms)?;
        Ok(next)
    }

    pub(super) fn latest(&self, outbox_id: &str) -> Option<&PublicationOutboxV1> {
        self.latest.get(outbox_id).map(|(record, _)| record)
    }

    pub(super) fn pending_recovery_actions(&self) -> Vec<(String, OutboxRecoveryActionV1)> {
        self.latest
            .iter()
            .filter_map(|(id, (record, _))| {
                let action = recovery_action(record);
                (action != OutboxRecoveryActionV1::Complete).then(|| (id.clone(), action))
            })
            .collect()
    }
}

fn validate_record_shape(
    record: &OutboxJournalRecordV1,
    expected_generation: u64,
    previous_journal_sha256: Option<&str>,
    previous_recorded_at_ms: Option<i64>,
    latest: &BTreeMap<String, (PublicationOutboxV1, String)>,
) -> Result<(), BoxError> {
    if record.schema_version != 1
        || record.generation != expected_generation
        || record.recorded_at_ms <= 0
        || previous_recorded_at_ms.is_some_and(|previous| record.recorded_at_ms <= previous)
        || record.previous_journal_record != optional_sha256(previous_journal_sha256)
    {
        return Err("schedule outbox: journal generation, predecessor, or time is invalid".into());
    }
    record.outbox.validate()?;
    match latest.get(&record.outbox.outbox_id) {
        None if record.outbox.state == PublicationOutboxStateV1::CreateIntent
            && record.outbox.previous_record == OptionalSha256V1::Absent
            && record.outbox.remote_observation_attempts == 0 =>
        {
            Ok(())
        }
        None => Err("schedule outbox: first identity record is not create-intent".into()),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_schedule_schema::GitObjectAlgorithmV1;
    use crate::compatibility_schedule_state::SchedulerStateRoot;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn identity(external_id: &str) -> OutboxRemoteIdentityV1 {
        OutboxRemoteIdentityV1 {
            repository: "owner/repository".into(),
            pull_request: 41,
            test_merge_oid: GitObjectIdV1 {
                algorithm: GitObjectAlgorithmV1::Sha1,
                hex: "a".repeat(40),
            },
            context: "a2a-bridge/compatibility".into(),
            app_id: "app-1".into(),
            external_id: external_id.into(),
        }
    }

    fn observation(
        source: &PublicationOutboxV1,
        check_run_id: u64,
        phase: RemoteCheckPhaseV1,
        observed_at_ms: i64,
    ) -> RemoteCheckObservationV1 {
        RemoteCheckObservationV1 {
            schema_version: 1,
            repository: source.repository.clone(),
            pull_request: source.pull_request,
            test_merge_oid: source.test_merge_oid.clone(),
            context: source.context.clone(),
            app_id: source.app_id.clone(),
            external_id: source.external_id.clone(),
            check_run_id,
            phase,
            observed_at_ms,
        }
    }

    fn terminal(conclusion: CheckConclusionV1) -> TerminalPublicationV1 {
        TerminalPublicationV1 {
            terminal_consumption_id: "consumption-1".into(),
            desired_conclusion: conclusion,
            evidence_set_sha256: "b".repeat(64),
            final_guard_sha256: "c".repeat(64),
        }
    }

    #[test]
    fn outbox_journal_recovers_every_crash_phase_without_remote_effects() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root.try_owner_admission("outbox-test").unwrap();
        let mut journal = PublicationOutboxJournal::open(&lock).unwrap();

        let create = journal.start(identity("external-1"), 10).unwrap();
        assert_eq!(
            journal.pending_recovery_actions(),
            vec![(
                create.outbox_id.clone(),
                OutboxRecoveryActionV1::ReconcileCreate
            )]
        );
        let create_unknown = journal.mark_create_unknown(&create.outbox_id, 11).unwrap();
        assert_eq!(
            recovery_action(&create_unknown),
            OutboxRecoveryActionV1::ReconcileCreate
        );
        let pending_observation = observation(&create, 77, RemoteCheckPhaseV1::InProgress, 12);
        let pending = journal
            .bind_remote_pending(&create.outbox_id, &pending_observation, 13)
            .unwrap();
        assert_eq!(
            recovery_action(&pending),
            OutboxRecoveryActionV1::AwaitTerminalPreparation
        );
        let prepared = journal
            .prepare_terminal(&create.outbox_id, terminal(CheckConclusionV1::Success), 14)
            .unwrap();
        assert_eq!(
            recovery_action(&prepared),
            OutboxRecoveryActionV1::PublishTerminalUpdate
        );
        let unknown = journal.mark_update_unknown(&create.outbox_id, 15).unwrap();
        assert_eq!(
            recovery_action(&unknown),
            OutboxRecoveryActionV1::ReconcileTerminalUpdate
        );
        let terminal_observation = observation(
            &unknown,
            77,
            RemoteCheckPhaseV1::Completed {
                conclusion: CheckConclusionV1::Success,
            },
            16,
        );
        let observed = journal
            .observe_terminal(&create.outbox_id, &terminal_observation, 17)
            .unwrap();
        assert_eq!(
            recovery_action(&observed),
            OutboxRecoveryActionV1::ConfirmObservedTerminal
        );
        let confirmed = journal.confirm(&create.outbox_id, 18).unwrap();
        assert_eq!(
            recovery_action(&confirmed),
            OutboxRecoveryActionV1::Complete
        );
        drop(journal);

        let reopened = PublicationOutboxJournal::open(&lock).unwrap();
        assert_eq!(reopened.latest(&create.outbox_id), Some(&confirmed));
        assert!(reopened.pending_recovery_actions().is_empty());
    }

    #[test]
    fn outbox_rejects_skips_identity_drift_and_conflicting_remote_terminal() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root.try_owner_admission("outbox-negative").unwrap();
        let mut journal = PublicationOutboxJournal::open(&lock).unwrap();
        let create = journal.start(identity("external-negative"), 20).unwrap();

        let mut skipped = create.clone();
        skipped.previous_record = OptionalSha256V1::Sha256 {
            value: outbox_payload_sha256(&create).unwrap(),
        };
        skipped.state = PublicationOutboxStateV1::Prepared;
        assert!(journal.append_exact(skipped, None, 21).is_err());

        let mut wrong_observation = observation(&create, 88, RemoteCheckPhaseV1::InProgress, 21);
        wrong_observation.external_id = "another-external".into();
        assert!(journal
            .bind_remote_pending(&create.outbox_id, &wrong_observation, 22)
            .is_err());

        let pending_observation = observation(&create, 88, RemoteCheckPhaseV1::InProgress, 22);
        journal
            .bind_remote_pending(&create.outbox_id, &pending_observation, 23)
            .unwrap();
        journal
            .prepare_terminal(&create.outbox_id, terminal(CheckConclusionV1::Failure), 24)
            .unwrap();
        let current = journal.latest(&create.outbox_id).unwrap().clone();
        let conflicting = observation(
            &current,
            88,
            RemoteCheckPhaseV1::Completed {
                conclusion: CheckConclusionV1::Success,
            },
            25,
        );
        assert!(journal
            .observe_terminal(&create.outbox_id, &conflicting, 26)
            .is_err());
        assert_eq!(
            journal.latest(&create.outbox_id).unwrap().state,
            PublicationOutboxStateV1::Prepared
        );
    }

    #[test]
    fn outbox_reopen_rejects_generation_gap_chain_tamper_and_replacement() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root.try_owner_admission("outbox-corruption").unwrap();
        let mut journal = PublicationOutboxJournal::open(&lock).unwrap();
        let create = journal.start(identity("external-corrupt"), 30).unwrap();
        journal.mark_create_unknown(&create.outbox_id, 31).unwrap();
        journal.mark_create_unknown(&create.outbox_id, 32).unwrap();
        drop(journal);

        let outbox_dir = lock.publication_outbox_directory().canonical_path();
        let second = outbox_dir.join(PublicationOutboxJournal::generation_name(2));
        let saved = std::fs::read(&second).unwrap();
        std::fs::remove_file(&second).unwrap();
        let error = PublicationOutboxJournal::open(&lock)
            .err()
            .expect("generation gap must fail closed");
        assert!(error.to_string().contains("contiguous"));

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut restored = options.open(&second).unwrap();
        restored.write_all(&saved).unwrap();
        restored.sync_all().unwrap();
        lock.publication_outbox_directory().sync().unwrap();

        let mut record: OutboxJournalRecordV1 = serde_json::from_slice(&saved).unwrap();
        record.previous_journal_record = OptionalSha256V1::Sha256 {
            value: "f".repeat(64),
        };
        let tampered = canonical_bytes(&record).unwrap();
        std::fs::write(&second, tampered).unwrap();
        std::fs::set_permissions(&second, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(PublicationOutboxJournal::open(&lock).is_err());
    }

    #[test]
    fn outbox_reopen_rederives_exact_persisted_remote_observation() {
        let directory = root();
        let root = SchedulerStateRoot::initialize_for_test(directory.path()).unwrap();
        let lock = root
            .try_owner_admission("outbox-observation-binding")
            .unwrap();
        let mut journal = PublicationOutboxJournal::open(&lock).unwrap();
        let create = journal.start(identity("external-observation"), 40).unwrap();
        let pending_observation = observation(&create, 99, RemoteCheckPhaseV1::InProgress, 41);
        journal
            .bind_remote_pending(&create.outbox_id, &pending_observation, 42)
            .unwrap();
        journal
            .prepare_terminal(&create.outbox_id, terminal(CheckConclusionV1::Success), 43)
            .unwrap();
        let current = journal.latest(&create.outbox_id).unwrap().clone();
        let terminal_observation = observation(
            &current,
            99,
            RemoteCheckPhaseV1::Completed {
                conclusion: CheckConclusionV1::Success,
            },
            44,
        );
        let observed = journal
            .observe_terminal(&create.outbox_id, &terminal_observation, 45)
            .unwrap();

        let mut changed_hash = observed.clone();
        changed_hash.previous_record = OptionalSha256V1::Sha256 {
            value: outbox_payload_sha256(&observed).unwrap(),
        };
        changed_hash.state = PublicationOutboxStateV1::Confirmed;
        changed_hash.remote_observation = OptionalSha256V1::Sha256 {
            value: "f".repeat(64),
        };
        assert!(journal.append_exact(changed_hash, None, 46).is_err());
        drop(journal);

        let outbox_dir = lock.publication_outbox_directory().canonical_path();
        let pending_record: OutboxJournalRecordV1 = serde_json::from_slice(
            &std::fs::read(outbox_dir.join(PublicationOutboxJournal::generation_name(2))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            pending_record.remote_observation.as_ref(),
            Some(&pending_observation)
        );

        let terminal_path = outbox_dir.join(PublicationOutboxJournal::generation_name(4));
        let mut terminal_record: OutboxJournalRecordV1 =
            serde_json::from_slice(&std::fs::read(&terminal_path).unwrap()).unwrap();
        let OptionalRemoteCheckObservationV1::Observation { value } =
            &mut terminal_record.remote_observation
        else {
            panic!("terminal transition must persist the exact observation")
        };
        value.phase = RemoteCheckPhaseV1::Completed {
            conclusion: CheckConclusionV1::Failure,
        };
        std::fs::write(&terminal_path, canonical_bytes(&terminal_record).unwrap()).unwrap();
        std::fs::set_permissions(&terminal_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(PublicationOutboxJournal::open(&lock).is_err());
    }

    #[test]
    fn outbox_source_contains_no_network_or_github_effect_client() {
        const SOURCE: &str = include_str!("compatibility_schedule_outbox.rs");
        for forbidden in [
            concat!("req", "west::"),
            concat!("octo", "crab::"),
            concat!("git", "hub::"),
            concat!("PO", "ST "),
            concat!("PAT", "CH "),
        ] {
            assert!(
                !SOURCE.contains(forbidden),
                "outbox gained an effect client: {forbidden}"
            );
        }
    }
}
