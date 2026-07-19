//! Crash-consistent R3d2 admission linearization and recovery.
//!
//! A complete owner-private admission commit is the only linearization point. Authority and
//! ledger journals are previewed before that commit and published idempotently after it. No type
//! in this module can call a provider; only the opaque capability created after publication may be
//! transferred to an injected runner handoff.

#![allow(dead_code)] // R3d5 activation will make the internal admitted-capability path live.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule::{
    load_schedule_foundation, EffectCapsV1, EffectClassV1, TriggerKindV1,
};
use crate::compatibility_schedule_admission::{
    rederive_claimed_support_ledger_context_from_foundation, rederive_manual_ledger_context,
    rederive_scheduled_ledger_context_from_foundation, CharacterizationStateV1,
    CompletedEquivalentEvidenceV1, ControlStateV1, DerivedLedgerAdmissionContextV1,
    EquivalentWorkDecisionV1, EquivalentWorkStateV1,
};
use crate::compatibility_schedule_authority::{
    authority_state_snapshot_sha256, manual_admission_sha256, select_characterization_authority,
    select_manual_accounting_grant, select_standing_grant, AuthorityEnvironmentV1,
    AuthorityJournalOpen, AuthorityStateModelV1, AuthorityStateSnapshotV1,
    CharacterizationAdmissionRequestV1, FileAuthorityJournal, OneShotLifecyclePhaseV1,
    SealedManualAdmissionV1, StandingAdmissionRequestV1,
};
use crate::compatibility_schedule_ledger::{
    prepared_reservation_sha256, validate_prepared_reservation_context, ConservativeChargeReasonV1,
    FileCompatibilityLedger, LedgerBudgetAuthorityV1, LedgerReservationRequestV1,
    ReconciliationDecisionV1,
};
use crate::compatibility_schedule_preflight::{
    pin_action_directories, preflight_pass_sha256, run_zero_effect_preflight,
    validate_planned_directory_binding, PinnedActionDirectoriesV1, PlannedDirectoryBindingV1,
    PreflightBindingV1, PreflightFenceV1, PreflightPassV1, ZeroEffectPreflightChecks,
};
use crate::compatibility_schedule_schema::{
    validate_supervisor_record, AdmissionAuthorityV1, AdmissionTriggerIdentityV1,
    CaseExecutionFingerprintInputV1, ClaimedSupportCharacterizationSourceV1,
    ConsumptionEvidenceProvenanceV1, ConsumptionRecordV1, DeadlineDerivationV1,
    EffectiveIdentityV1, EquivalentWorkReservationV1, LedgerReservationV1,
    OptionalChildArtifactRefV1, OptionalSha256V1, OptionalSupervisorOutcomeV1, OptionalTextV1,
    ScheduledExecutionSourceV1, SupervisorPhaseV1, SupervisorRecordV1, SupervisorTerminalOutcomeV1,
    UsageChargeV1, ValidateRecord,
};
use crate::compatibility_schedule_state::{AdmissionStateCapability, AuthorityStateCapability};
use crate::compatibility_schedule_supervisor::{
    ensure_prepared_supervisor, HardDeadline, PreparedSupervisorV1, VerifiedChildTerminalProofV1,
};
use crate::{local_file, BoxError};

const MAX_COMMIT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COMMIT_GENERATIONS: usize = 100_000;
const COMMIT_PREFIX: &str = "admission-commit.";
const TERMINAL_PREFIX: &str = "admission-terminal.";

fn transaction_hash<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| format!("schedule transaction: cannot canonicalize {label}: {error}"))?;
    let mut bytes = format!("a2a-bridge:r3d2:transaction:{label}:v1\0").into_bytes();
    bytes.extend_from_slice(&canonical);
    Ok(local_file::sha256_hex(&bytes))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, BoxError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_COMMIT_BYTES {
        return Err("schedule transaction: admission commit exceeds the byte bound".into());
    }
    Ok(bytes)
}

fn stable_id(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.len() > 128
        || !matches!(
            value.as_bytes().first(),
            Some(b'a'..=b'z') | Some(b'0'..=b'9')
        )
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(format!("schedule transaction: {label} is not a bounded stable id").into());
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionStateV1 {
    pub(super) equivalent_work: EquivalentWorkStateV1,
    pub(super) characterizations: CharacterizationStateV1,
    pub(super) controls: ControlStateV1,
}

impl AdmissionStateV1 {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn validate(&self) -> Result<(), BoxError> {
        self.equivalent_work.validate()?;
        self.characterizations.validate()?;
        self.controls.validate()
    }
}

fn admission_state_sha256(value: &AdmissionStateV1) -> Result<String, BoxError> {
    value.validate()?;
    transaction_hash("admission-state", value)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum InternalAdmissionSourceKindV1 {
    Scheduled,
    ClaimedSupportCharacterization,
    GenericManual,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizedEffectEnvelopeV1 {
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) caps: EffectCapsV1,
}

impl AuthorizedEffectEnvelopeV1 {
    fn validate(
        &self,
        context: &DerivedLedgerAdmissionContextV1,
        action: &AuthorityCommitActionV1,
        authority: &AuthorityStateSnapshotV1,
    ) -> Result<(), BoxError> {
        let mut canonical = self.allowed_effects.clone();
        canonical.sort();
        canonical.dedup();
        self.caps.validate("admitted effect caps")?;
        if canonical.is_empty()
            || canonical != self.allowed_effects
            || self.caps != context.identities.case_execution.input.actual_caps
        {
            return Err("schedule transaction: admitted effect envelope is noncanonical".into());
        }
        match action {
            AuthorityCommitActionV1::Manual { admission }
                if self.allowed_effects == admission.record.allowed_effects
                    && self.caps == admission.record.caps =>
            {
                Ok(())
            }
            AuthorityCommitActionV1::OneShot { entry_id } => {
                let entry = authority
                    .state
                    .authorizations
                    .values()
                    .flat_map(|authorization| authorization.entries.iter())
                    .find(|entry| &entry.entry_id == entry_id)
                    .ok_or("schedule transaction: admitted one-shot entry is absent")?;
                if self.allowed_effects == entry.allowed_effects && self.caps == entry.caps {
                    Ok(())
                } else {
                    Err("schedule transaction: one-shot effect envelope diverged".into())
                }
            }
            AuthorityCommitActionV1::Standing => {
                let AdmissionAuthorityV1::StandingGrant(bound) =
                    &context.identities.admission_attempt.input.authority
                else {
                    return Err("schedule transaction: standing effect authority is absent".into());
                };
                let grant = authority
                    .state
                    .grants
                    .get(&bound.grant_id)
                    .ok_or("schedule transaction: standing grant is absent")?;
                let profile = grant
                    .profiles
                    .iter()
                    .find(|profile| {
                        profile.case_id == context.case_id
                            && profile.provider_family == context.provider_family
                            && profile.characterization_id == bound.characterization_id
                            && profile.characterization_sha256 == bound.characterization_sha256
                    })
                    .ok_or("schedule transaction: characterized standing profile is absent")?;
                if self
                    .allowed_effects
                    .iter()
                    .all(|effect| grant.allowed_effects.contains(effect))
                {
                    self.caps
                        .within(&grant.per_run_caps, "admitted standing caps")?;
                    self.caps
                        .within(&profile.caps, "admitted characterized caps")?;
                    Ok(())
                } else {
                    Err("schedule transaction: standing effects exceed the grant".into())
                }
            }
            _ => Err("schedule transaction: effect envelope and authority arm disagree".into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AuthorityCommitActionV1 {
    Standing,
    OneShot {
        entry_id: String,
    },
    Manual {
        admission: Box<SealedManualAdmissionV1>,
    },
}

impl AuthorityCommitActionV1 {
    fn validate_binding(
        &self,
        context: &DerivedLedgerAdmissionContextV1,
        commit_identity_sha256: &str,
        recorded_at_ms: i64,
        authority_after: &AuthorityStateSnapshotV1,
    ) -> Result<(), BoxError> {
        let authority = &context.identities.admission_attempt.input.authority;
        match (self, authority) {
            (Self::Standing, AdmissionAuthorityV1::StandingGrant(_)) => Ok(()),
            (Self::OneShot { entry_id }, AdmissionAuthorityV1::CharacterizationOnce(selected))
                if entry_id == &selected.entry_id =>
            {
                stable_id("one-shot entry id", entry_id)?;
                let lifecycle = authority_after
                    .state
                    .one_shots
                    .get(entry_id)
                    .ok_or("schedule transaction: committed one-shot lifecycle is absent")?;
                let reflected = matches!(
                    &lifecycle.phase,
                    OneShotLifecyclePhaseV1::ConsumedUnreconciled {
                        admission_commit_sha256,
                        consumed_at_ms,
                    } if admission_commit_sha256 == commit_identity_sha256
                        && *consumed_at_ms == recorded_at_ms
                ) || matches!(
                    &lifecycle.phase,
                    OneShotLifecyclePhaseV1::Reconciled {
                        admission_commit_sha256,
                        consumed_at_ms,
                        ..
                    } if admission_commit_sha256 == commit_identity_sha256
                        && *consumed_at_ms == recorded_at_ms
                );
                if reflected {
                    Ok(())
                } else {
                    Err("schedule transaction: one-shot consumption is not reflected".into())
                }
            }
            (Self::Manual { admission }, AdmissionAuthorityV1::ManualAcknowledgement(selected))
                if &admission.authority == authority =>
            {
                crate::compatibility_schedule_authority::validate_sealed_manual_admission(
                    admission,
                )?;
                let consumed = authority_after
                    .state
                    .manual_admissions
                    .get(&selected.request_nonce)
                    .ok_or("schedule transaction: manual consumption is absent")?;
                if consumed.record == admission.record
                    && consumed.authority == *selected
                    && consumed.admission_commit_sha256 == commit_identity_sha256
                    && consumed.consumed_at_ms == recorded_at_ms
                {
                    Ok(())
                } else {
                    Err("schedule transaction: manual consumption is not reflected".into())
                }
            }
            _ => Err("schedule transaction: authority action and admission arm disagree".into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AdmissionDispositionV1 {
    Reserved {
        equivalent_work: EquivalentWorkReservationV1,
        ledger: Box<LedgerReservationV1>,
        ledger_sha256: String,
        deadline_derivation: Box<DeadlineDerivationV1>,
        supervisor: Box<SupervisorRecordV1>,
        supervisor_sha256: String,
    },
    Reused {
        consumption: ConsumptionRecordV1,
    },
}

fn supervisor_record_sha256(value: &SupervisorRecordV1) -> Result<String, BoxError> {
    validate_supervisor_record(value)?;
    transaction_hash("prepared-supervisor", value)
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct AdmissionCommitIdentityInputV1<'a> {
    schema_version: u16,
    generation: u64,
    previous_commit: &'a OptionalSha256V1,
    source_kind: InternalAdmissionSourceKindV1,
    source_sha256: &'a str,
    context: &'a DerivedLedgerAdmissionContextV1,
    authority_action: &'a AuthorityCommitActionV1,
    effect_envelope: &'a AuthorizedEffectEnvelopeV1,
    admission_state_after_sha256: &'a str,
    disposition: &'a AdmissionDispositionV1,
    initial_preflight_sha256: &'a str,
    final_preflight_sha256: &'a str,
    trusted_root: &'a PlannedDirectoryBindingV1,
    requested_cwd: &'a PlannedDirectoryBindingV1,
    terminal_deadline_ms: i64,
    recorded_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionCommitV1 {
    pub(super) schema_version: u16,
    pub(super) generation: u64,
    pub(super) previous_commit: OptionalSha256V1,
    pub(super) commit_id: String,
    pub(super) commit_identity_sha256: String,
    pub(super) source_kind: InternalAdmissionSourceKindV1,
    pub(super) source_sha256: String,
    pub(super) context: DerivedLedgerAdmissionContextV1,
    pub(super) authority_action: AuthorityCommitActionV1,
    pub(super) effect_envelope: AuthorizedEffectEnvelopeV1,
    pub(super) authority_before_snapshot_sha256: String,
    pub(super) authority_after_snapshot: AuthorityStateSnapshotV1,
    pub(super) authority_after_snapshot_sha256: String,
    pub(super) admission_state_before_sha256: String,
    pub(super) admission_state_after: AdmissionStateV1,
    pub(super) disposition: AdmissionDispositionV1,
    pub(super) initial_preflight: PreflightPassV1,
    pub(super) final_preflight: PreflightPassV1,
    pub(super) trusted_root: PlannedDirectoryBindingV1,
    pub(super) requested_cwd: PlannedDirectoryBindingV1,
    pub(super) terminal_deadline_ms: i64,
    pub(super) recorded_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AdmissionTerminalDispositionV1 {
    ProvedPreEffect {
        evidence_sha256: String,
    },
    ValidTerminal {
        evidence: Box<CompletedEquivalentEvidenceV1>,
        usage: UsageChargeV1,
        prompt_was_accepted: bool,
    },
    Conservative {
        evidence_sha256: String,
        reason: ConservativeChargeReasonV1,
        prompt_may_have_been_accepted: bool,
    },
}

#[derive(Clone, Debug)]
pub(super) enum AdmissionTerminalProofV1 {
    ProvedPreEffect {
        evidence_sha256: String,
    },
    ValidTerminal {
        child: Box<VerifiedChildTerminalProofV1>,
    },
    Conservative {
        evidence_sha256: String,
        reason: ConservativeChargeReasonV1,
        prompt_may_have_been_accepted: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionTerminalV1 {
    pub(super) schema_version: u16,
    pub(super) generation: u64,
    pub(super) terminal_id: String,
    pub(super) admission_commit_identity_sha256: String,
    pub(super) supervisor: SupervisorRecordV1,
    pub(super) supervisor_journal_sha256: String,
    pub(super) disposition: AdmissionTerminalDispositionV1,
    pub(super) admission_state_before_sha256: String,
    pub(super) admission_state_after: AdmissionStateV1,
    pub(super) recorded_at_ms: i64,
}

fn supervisor_outcome(value: &SupervisorRecordV1) -> Option<SupervisorTerminalOutcomeV1> {
    match value.outcome {
        OptionalSupervisorOutcomeV1::Absent => None,
        OptionalSupervisorOutcomeV1::Outcome { value } => Some(value),
    }
}

fn usage_within_caps(usage: &UsageChargeV1, caps: &EffectCapsV1) -> bool {
    usage.attempts == 1
        && usage.attempts <= caps.attempts
        && usage.tokens <= caps.max_tokens
        && usage.cost_microusd <= caps.max_cost_microusd
        && usage.elapsed_millis <= caps.timeout_secs.saturating_mul(1_000)
}

fn pre_effect_evidence(
    commit: &AdmissionCommitV1,
    reservation: &EquivalentWorkReservationV1,
    evidence_sha256: String,
    terminal_at_ms: i64,
) -> CompletedEquivalentEvidenceV1 {
    let expected = commit
        .context
        .identities
        .case_execution
        .input
        .expected_effective_identity
        .clone();
    CompletedEquivalentEvidenceV1 {
        reservation_id: reservation.reservation_id.clone(),
        evidence_sha256,
        satisfied_purpose: reservation.evidence_purpose,
        freshness_bucket: reservation.freshness_bucket.clone(),
        characterization_profile: reservation.characterization_profile.clone(),
        case_execution: reservation.case_execution.clone(),
        expected_effective_identity: expected.clone(),
        observed_effective_identity: expected,
        provenance: ConsumptionEvidenceProvenanceV1::Ordinary,
        reusable: false,
        terminal_at_ms,
    }
}

fn validate_terminal_against_state(
    value: &AdmissionTerminalV1,
    commit: &AdmissionCommitV1,
    before: &AdmissionStateV1,
) -> Result<(), BoxError> {
    if value.schema_version != 1
        || value.generation != commit.generation
        || value.recorded_at_ms < commit.recorded_at_ms
        || value.terminal_id != format!("terminal-{}", commit.commit_identity_sha256)
        || value.admission_commit_identity_sha256 != commit.commit_identity_sha256
        || !local_file::valid_sha256(&value.supervisor_journal_sha256)
        || value.admission_state_before_sha256 != admission_state_sha256(before)?
    {
        return Err("schedule transaction: terminal identity or state binding diverged".into());
    }
    let AdmissionDispositionV1::Reserved {
        equivalent_work,
        ledger,
        supervisor: prepared,
        ..
    } = &commit.disposition
    else {
        return Err("schedule transaction: reused admission cannot have a terminal record".into());
    };
    validate_supervisor_record(&value.supervisor)?;
    if value.supervisor.supervisor_record_id != prepared.supervisor_record_id
        || value.supervisor.run_id != prepared.run_id
        || value.supervisor.window_id != prepared.window_id
        || value.supervisor.trigger != prepared.trigger
        || value.supervisor.recorded_at_ms > value.recorded_at_ms
        || !matches!(
            value.supervisor.phase,
            SupervisorPhaseV1::Complete | SupervisorPhaseV1::SafetyHold
        )
    {
        return Err("schedule transaction: terminal supervisor/reservation join diverged".into());
    }

    let mut expected = before.clone();
    match &value.disposition {
        AdmissionTerminalDispositionV1::ProvedPreEffect { evidence_sha256 } => {
            if !local_file::valid_sha256(evidence_sha256)
                || supervisor_outcome(&value.supervisor)
                    != Some(SupervisorTerminalOutcomeV1::CancelledBeforeRunning)
            {
                return Err("schedule transaction: pre-effect terminal is not proved".into());
            }
            expected
                .equivalent_work
                .record_completed(pre_effect_evidence(
                    commit,
                    equivalent_work,
                    evidence_sha256.clone(),
                    value.recorded_at_ms,
                ))?;
        }
        AdmissionTerminalDispositionV1::ValidTerminal {
            evidence,
            usage,
            prompt_was_accepted: _,
        } => {
            if value.supervisor.phase != SupervisorPhaseV1::Complete
                || supervisor_outcome(&value.supervisor)
                    == Some(SupervisorTerminalOutcomeV1::CancelledBeforeRunning)
                || !usage_within_caps(usage, &ledger.caps)
                || evidence.reservation_id != equivalent_work.reservation_id
                || evidence.characterization_profile != equivalent_work.characterization_profile
                || evidence.case_execution != equivalent_work.case_execution
                || evidence.satisfied_purpose != equivalent_work.evidence_purpose
                || evidence.freshness_bucket != equivalent_work.freshness_bucket
                || evidence.expected_effective_identity
                    != commit
                        .context
                        .identities
                        .case_execution
                        .input
                        .expected_effective_identity
                || evidence.expected_effective_identity != evidence.observed_effective_identity
                || evidence.terminal_at_ms > value.recorded_at_ms
            {
                return Err("schedule transaction: valid terminal evidence diverged".into());
            }
            expected
                .equivalent_work
                .record_completed(evidence.as_ref().clone())?;
        }
        AdmissionTerminalDispositionV1::Conservative {
            evidence_sha256, ..
        } => {
            if !local_file::valid_sha256(evidence_sha256)
                || supervisor_outcome(&value.supervisor)
                    == Some(SupervisorTerminalOutcomeV1::CancelledBeforeRunning)
            {
                return Err("schedule transaction: conservative terminal shape diverged".into());
            }
            // Ambiguous/possibly accepted work intentionally remains live and blocks a successor.
        }
    }
    expected.validate()?;
    if value.admission_state_after != expected {
        return Err("schedule transaction: terminal state was not reducer-derived".into());
    }
    Ok(())
}

fn commit_identity_sha256(
    value: &AdmissionCommitV1,
    admission_state_after_sha256: &str,
    initial_preflight_sha256: &str,
    final_preflight_sha256: &str,
) -> Result<String, BoxError> {
    transaction_hash(
        "admission-commit-identity",
        &AdmissionCommitIdentityInputV1 {
            schema_version: 1,
            generation: value.generation,
            previous_commit: &value.previous_commit,
            source_kind: value.source_kind,
            source_sha256: &value.source_sha256,
            context: &value.context,
            authority_action: &value.authority_action,
            effect_envelope: &value.effect_envelope,
            admission_state_after_sha256,
            disposition: &value.disposition,
            initial_preflight_sha256,
            final_preflight_sha256,
            trusted_root: &value.trusted_root,
            requested_cwd: &value.requested_cwd,
            terminal_deadline_ms: value.terminal_deadline_ms,
            recorded_at_ms: value.recorded_at_ms,
        },
    )
}

fn validate_commit_against_state(
    value: &AdmissionCommitV1,
    before: &AdmissionStateV1,
) -> Result<(), BoxError> {
    if value.schema_version != 1
        || value.generation == 0
        || value.recorded_at_ms <= 0
        || value.terminal_deadline_ms < value.recorded_at_ms
    {
        return Err("schedule transaction: commit header is malformed".into());
    }
    stable_id("commit id", &value.commit_id)?;
    if !local_file::valid_sha256(&value.source_sha256)
        || !local_file::valid_sha256(&value.authority_before_snapshot_sha256)
        || !local_file::valid_sha256(&value.authority_after_snapshot_sha256)
        || !local_file::valid_sha256(&value.commit_identity_sha256)
    {
        return Err("schedule transaction: commit contains a malformed digest".into());
    }
    value.context.identities.validate()?;
    stable_id("case id", &value.context.case_id)?;
    stable_id("provider family", &value.context.provider_family)?;
    value.authority_after_snapshot.validate()?;
    if value.authority_after_snapshot_sha256
        != authority_state_snapshot_sha256(&value.authority_after_snapshot)?
    {
        return Err("schedule transaction: previewed authority hash diverged".into());
    }
    value.admission_state_after.validate()?;
    validate_planned_directory_binding(&value.trusted_root)?;
    validate_planned_directory_binding(&value.requested_cwd)?;
    let (preflight_ledger, preflight_supervisor, preflight_deadline) = match &value.disposition {
        AdmissionDispositionV1::Reserved {
            ledger,
            supervisor,
            deadline_derivation,
            ..
        } => (
            Some(ledger.as_ref()),
            Some(supervisor.as_ref()),
            Some(deadline_derivation.as_ref()),
        ),
        AdmissionDispositionV1::Reused { .. } => (None, None, None),
    };
    let expected_preflight_binding = admission_preflight_binding(
        value.source_kind,
        &value.source_sha256,
        &value.context,
        &value.authority_action,
        &value.effect_envelope,
        &value.authority_before_snapshot_sha256,
        preflight_ledger,
        preflight_supervisor,
        preflight_deadline,
        &value.trusted_root,
        &value.requested_cwd,
        value.terminal_deadline_ms,
        value.recorded_at_ms,
    )?;
    if value.initial_preflight.fence != PreflightFenceV1::Initial
        || value.final_preflight.fence != PreflightFenceV1::Final
        || value.initial_preflight.binding != expected_preflight_binding
        || value.final_preflight.binding != expected_preflight_binding
        || value.initial_preflight.completed_at_ms > value.final_preflight.completed_at_ms
        || value.final_preflight.completed_at_ms > value.recorded_at_ms
    {
        return Err("schedule transaction: preflight fence identities or ordering diverged".into());
    }
    let initial_preflight_sha256 = preflight_pass_sha256(&value.initial_preflight)?;
    let final_preflight_sha256 = preflight_pass_sha256(&value.final_preflight)?;
    let before_sha256 = admission_state_sha256(before)?;
    let after_sha256 = admission_state_sha256(&value.admission_state_after)?;
    if value.admission_state_before_sha256 != before_sha256
        || value.commit_id
            != format!(
                "admission-{}",
                value.context.identities.attempt_idempotency_key
            )
        || value.commit_identity_sha256
            != commit_identity_sha256(
                value,
                &after_sha256,
                &initial_preflight_sha256,
                &final_preflight_sha256,
            )?
    {
        return Err("schedule transaction: commit identity or state binding diverged".into());
    }
    value.authority_action.validate_binding(
        &value.context,
        &value.commit_identity_sha256,
        value.recorded_at_ms,
        &value.authority_after_snapshot,
    )?;
    value.effect_envelope.validate(
        &value.context,
        &value.authority_action,
        &value.authority_after_snapshot,
    )?;
    if value.authority_after_snapshot.recorded_at_ms != value.recorded_at_ms
        || value.authority_after_snapshot.previous_record
            != (OptionalSha256V1::Sha256 {
                value: value.authority_before_snapshot_sha256.clone(),
            })
    {
        return Err("schedule transaction: previewed authority generation is not bound".into());
    }

    let mut expected_state = before.clone();
    let expected_decision = expected_state.equivalent_work.reserve_or_reuse(
        &value.context.identities,
        value
            .context
            .identities
            .admission_attempt
            .input
            .authority
            .clone(),
        value.recorded_at_ms,
    )?;
    if expected_state != value.admission_state_after {
        return Err("schedule transaction: admission state changed outside one reducer".into());
    }
    match (&value.disposition, expected_decision) {
        (
            AdmissionDispositionV1::Reserved {
                equivalent_work,
                ledger,
                ledger_sha256,
                deadline_derivation,
                supervisor,
                supervisor_sha256,
            },
            EquivalentWorkDecisionV1::Reserved(expected),
        ) if equivalent_work == &expected => {
            validate_prepared_reservation_context(ledger, &value.context)?;
            validate_deadline_record_binding(
                &value.context,
                ledger,
                supervisor,
                deadline_derivation,
                value.terminal_deadline_ms,
                value.recorded_at_ms,
            )?;
            if ledger.reserved_at_ms != value.recorded_at_ms
                || ledger_sha256 != &prepared_reservation_sha256(ledger)?
                || supervisor_sha256 != &supervisor_record_sha256(supervisor)?
                || supervisor.trigger
                    != value
                        .context
                        .identities
                        .admission_attempt
                        .input
                        .trigger
                        .kind
                || supervisor.window_id
                    != value
                        .context
                        .identities
                        .admission_attempt
                        .input
                        .trigger
                        .window_id
            {
                return Err("schedule transaction: reserved disposition binding diverged".into());
            }
        }
        (
            AdmissionDispositionV1::Reused { consumption },
            EquivalentWorkDecisionV1::Reused(expected),
        ) if consumption == &expected => {
            if !matches!(
                value.context.identities.admission_attempt.input.authority,
                AdmissionAuthorityV1::StandingGrant(_)
            ) || !matches!(value.authority_action, AuthorityCommitActionV1::Standing)
            {
                return Err(
                    "schedule transaction: one-shot/manual work cannot reuse evidence".into(),
                );
            }
        }
        _ => {
            return Err("schedule transaction: recorded disposition is not reducer-derived".into())
        }
    }
    Ok(())
}

pub(super) struct AdmissionCommitProposalV1 {
    pub(super) source_kind: InternalAdmissionSourceKindV1,
    pub(super) source_sha256: String,
    pub(super) authority_snapshot_sha256: String,
    pub(super) context: DerivedLedgerAdmissionContextV1,
    pub(super) authority_action: AuthorityCommitActionV1,
    pub(super) effect_envelope: AuthorizedEffectEnvelopeV1,
    pub(super) ledger: Option<LedgerReservationV1>,
    pub(super) supervisor: Option<PreparedSupervisorV1>,
    pub(super) initial_preflight: PreflightPassV1,
    pub(super) final_preflight: PreflightPassV1,
    pub(super) trusted_root: PlannedDirectoryBindingV1,
    pub(super) requested_cwd: PlannedDirectoryBindingV1,
    pub(super) terminal_deadline_ms: i64,
    pub(super) recorded_at_ms: i64,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct AdmissionPreflightSubjectInputV1<'a> {
    schema_version: u16,
    source_kind: InternalAdmissionSourceKindV1,
    source_sha256: &'a str,
    context: &'a DerivedLedgerAdmissionContextV1,
    authority_action: &'a AuthorityCommitActionV1,
    effect_envelope: &'a AuthorizedEffectEnvelopeV1,
    authority_snapshot_sha256: &'a str,
    ledger_sha256: Option<&'a str>,
    supervisor_sha256: Option<&'a str>,
    deadline_derivation_sha256: Option<&'a str>,
    trusted_root: &'a PlannedDirectoryBindingV1,
    requested_cwd: &'a PlannedDirectoryBindingV1,
    terminal_deadline_ms: i64,
    commit_at_ms: i64,
}

#[allow(clippy::too_many_arguments)]
fn admission_preflight_binding(
    source_kind: InternalAdmissionSourceKindV1,
    source_sha256: &str,
    context: &DerivedLedgerAdmissionContextV1,
    authority_action: &AuthorityCommitActionV1,
    effect_envelope: &AuthorizedEffectEnvelopeV1,
    authority_snapshot_sha256: &str,
    ledger: Option<&LedgerReservationV1>,
    supervisor: Option<&SupervisorRecordV1>,
    deadline_derivation: Option<&DeadlineDerivationV1>,
    trusted_root: &PlannedDirectoryBindingV1,
    requested_cwd: &PlannedDirectoryBindingV1,
    terminal_deadline_ms: i64,
    commit_at_ms: i64,
) -> Result<PreflightBindingV1, BoxError> {
    if !local_file::valid_sha256(authority_snapshot_sha256)
        || commit_at_ms <= 0
        || terminal_deadline_ms < commit_at_ms
    {
        return Err("schedule transaction: preflight authority/time binding is malformed".into());
    }
    validate_planned_directory_binding(trusted_root)?;
    validate_planned_directory_binding(requested_cwd)?;
    let ledger_sha256 = ledger.map(prepared_reservation_sha256).transpose()?;
    let supervisor_sha256 = supervisor.map(supervisor_record_sha256).transpose()?;
    let deadline_derivation_sha256 = deadline_derivation
        .map(|value| {
            value.validate()?;
            Ok::<_, BoxError>(value.derivation.sha256.clone())
        })
        .transpose()?;
    let subject = AdmissionPreflightSubjectInputV1 {
        schema_version: 1,
        source_kind,
        source_sha256,
        context,
        authority_action,
        effect_envelope,
        authority_snapshot_sha256,
        ledger_sha256: ledger_sha256.as_deref(),
        supervisor_sha256: supervisor_sha256.as_deref(),
        deadline_derivation_sha256: deadline_derivation_sha256.as_deref(),
        trusted_root,
        requested_cwd,
        terminal_deadline_ms,
        commit_at_ms,
    };
    Ok(PreflightBindingV1 {
        schema_version: 1,
        admission_subject_sha256: transaction_hash("preflight-admission-subject", &subject)?,
        authority_snapshot_sha256: authority_snapshot_sha256.to_owned(),
        trusted_root_binding_sha256: transaction_hash("preflight-trusted-root", trusted_root)?,
        requested_cwd_binding_sha256: transaction_hash("preflight-requested-cwd", requested_cwd)?,
        commit_at_ms,
    })
}

fn validate_deadline_record_binding(
    context: &DerivedLedgerAdmissionContextV1,
    ledger: &LedgerReservationV1,
    supervisor: &SupervisorRecordV1,
    record: &DeadlineDerivationV1,
    terminal_deadline_ms: i64,
    recorded_at_ms: i64,
) -> Result<(), BoxError> {
    record.validate()?;
    validate_supervisor_record(supervisor)?;
    let trigger = &context.identities.admission_attempt.input.trigger;
    let authority_remaining_ms = terminal_deadline_ms
        .checked_sub(recorded_at_ms)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or("schedule transaction: authority terminal deadline is already exhausted")?;
    let selected = &record.input.budgets.selected_cases;
    let case_timeout_cap_ms = ledger
        .caps
        .timeout_secs
        .checked_mul(1_000)
        .ok_or("schedule transaction: ledger case timeout overflows")?;
    if supervisor.run_id != trigger.attempt_id
        || supervisor.window_id != trigger.window_id
        || record.input.run_id != supervisor.run_id
        || record.input.window_id != supervisor.window_id
        || supervisor.deadline_derivation_sha256 != record.derivation.sha256
        || selected.len() != 1
        || selected[0].case_id != context.case_id
        || selected[0].timeout_ms > case_timeout_cap_ms
        || record.input.remaining_at_derivation_ms > authority_remaining_ms
    {
        return Err(
            "schedule transaction: supervisor deadline is not contained by this admission".into(),
        );
    }
    Ok(())
}

fn validate_admission_deadline(
    context: &DerivedLedgerAdmissionContextV1,
    ledger: &LedgerReservationV1,
    supervisor: &SupervisorRecordV1,
    deadline: &HardDeadline,
    terminal_deadline_ms: i64,
    recorded_at_ms: i64,
) -> Result<(), BoxError> {
    validate_deadline_record_binding(
        context,
        ledger,
        supervisor,
        deadline.record(),
        terminal_deadline_ms,
        recorded_at_ms,
    )?;
    if deadline.remaining().is_zero() {
        return Err("schedule transaction: executable hard deadline is already exhausted".into());
    }
    Ok(())
}

pub(super) struct RederivedSourceAdmissionV1 {
    source_kind: InternalAdmissionSourceKindV1,
    source_sha256: String,
    context: DerivedLedgerAdmissionContextV1,
    authority_action: AuthorityCommitActionV1,
    effect_envelope: AuthorizedEffectEnvelopeV1,
    budget_authority: LedgerBudgetAuthorityV1,
    selected_at_ms: i64,
    terminal_deadline_ms: i64,
    authority_snapshot_sha256: String,
}

impl RederivedSourceAdmissionV1 {
    fn bind_authority_snapshot(mut self, sha256: &str) -> Result<Self, BoxError> {
        if !local_file::valid_sha256(sha256) {
            return Err(
                "schedule transaction: selected authority snapshot hash is malformed".into(),
            );
        }
        self.authority_snapshot_sha256 = sha256.into();
        Ok(self)
    }
}

fn selected_one_shot_budget(
    state: &AuthorityStateModelV1,
    context: &DerivedLedgerAdmissionContextV1,
    entry_id: &str,
) -> Result<LedgerBudgetAuthorityV1, BoxError> {
    let entry = state
        .authorizations
        .values()
        .flat_map(|authorization| authorization.entries.iter())
        .find(|entry| entry.entry_id == entry_id)
        .ok_or("schedule transaction: selected one-shot budget entry is absent")?;
    if entry.characterization_profile != context.identities.characterization_profile
        || entry.characterization_execution != context.identities.case_execution.fingerprint
        || entry.provider_family != context.provider_family
        || entry.caps != context.identities.case_execution.input.actual_caps
    {
        return Err("schedule transaction: selected one-shot budget bindings diverged".into());
    }
    Ok(LedgerBudgetAuthorityV1::CharacterizationOnce {
        entry_sha256: entry.entry_sha256.clone(),
        case_id: context.case_id.clone(),
        provider_family: context.provider_family.clone(),
        caps: entry.caps.clone(),
    })
}

fn rederive_scheduled_standing_source_against_state(
    state: &AuthorityStateModelV1,
    foundation_root: &Path,
    source: &ScheduledExecutionSourceV1,
    freshness_bucket: String,
    environment: &AuthorityEnvironmentV1,
    grant_id: &str,
    request: &StandingAdmissionRequestV1,
) -> Result<RederivedSourceAdmissionV1, BoxError> {
    let foundation = load_schedule_foundation(foundation_root)?;
    let context =
        rederive_scheduled_ledger_context_from_foundation(&foundation, source, freshness_bucket)?;
    let binding = foundation
        .scheduled_profiles
        .get(&source.source.row_id)
        .ok_or("schedule transaction: scheduled foundation row disappeared")?;
    if !matches!(
        source.trigger,
        TriggerKindV1::Daily | TriggerKindV1::ScheduledMain | TriggerKindV1::TestMerge
    ) || request.trigger != source.trigger
        || request.case_id != context.case_id
        || request.provider_family != context.provider_family
        || request.source != source.source
        || request.characterization_profile_sha256
            != context.identities.characterization_profile.sha256
        || request.allowed_effects != binding.allowed_effects
        || request.caps != source.caps
    {
        return Err("schedule transaction: scheduled request and source bindings diverged".into());
    }
    let selected = select_standing_grant(state, grant_id, environment, request)?;
    if selected != source.authority
        || selected != context.identities.admission_attempt.input.authority
    {
        return Err(
            "schedule transaction: scheduled source carries stale standing authority".into(),
        );
    }
    let AdmissionAuthorityV1::StandingGrant(bound) = &selected else {
        return Err("schedule transaction: scheduled source selected a non-standing arm".into());
    };
    let grant = state
        .grants
        .get(grant_id)
        .ok_or("schedule transaction: selected standing grant disappeared")?;
    if bound.grant_sha256 != grant.grant_sha256 {
        return Err("schedule transaction: selected standing grant hash diverged".into());
    }
    Ok(RederivedSourceAdmissionV1 {
        source_kind: InternalAdmissionSourceKindV1::Scheduled,
        source_sha256: source.source_sha256.clone(),
        context,
        authority_action: AuthorityCommitActionV1::Standing,
        effect_envelope: AuthorizedEffectEnvelopeV1 {
            allowed_effects: binding.allowed_effects.clone(),
            caps: source.caps.clone(),
        },
        budget_authority: LedgerBudgetAuthorityV1::StandingGrant {
            grant_sha256: grant.grant_sha256.clone(),
            budgets: grant.budgets.clone(),
        },
        selected_at_ms: environment.now_ms,
        terminal_deadline_ms: environment.terminal_deadline_ms,
        authority_snapshot_sha256: String::new(),
    })
}

fn rederive_scheduled_characterization_source_against_state(
    state: &AuthorityStateModelV1,
    foundation_root: &Path,
    source: &ScheduledExecutionSourceV1,
    freshness_bucket: String,
    environment: &AuthorityEnvironmentV1,
    authorization_id: &str,
    request: &CharacterizationAdmissionRequestV1,
) -> Result<RederivedSourceAdmissionV1, BoxError> {
    let foundation = load_schedule_foundation(foundation_root)?;
    let context =
        rederive_scheduled_ledger_context_from_foundation(&foundation, source, freshness_bucket)?;
    let binding = foundation
        .scheduled_profiles
        .get(&source.source.row_id)
        .ok_or("schedule transaction: scheduled characterization row disappeared")?;
    if source.trigger != TriggerKindV1::ManualCharacterization
        || request.source != source.source
        || request.characterization_profile_sha256
            != context.identities.characterization_profile.sha256
        || request.characterization_execution_sha256
            != context.identities.case_execution.fingerprint.sha256
        || request.provider_family != context.provider_family
        || request.allowed_effects != binding.allowed_effects
        || request.caps != source.caps
    {
        return Err("schedule transaction: scheduled characterization bindings diverged".into());
    }
    let selected =
        select_characterization_authority(state, authorization_id, environment, request)?;
    if selected != source.authority
        || selected != context.identities.admission_attempt.input.authority
    {
        return Err(
            "schedule transaction: scheduled source carries stale one-shot authority".into(),
        );
    }
    let AdmissionAuthorityV1::CharacterizationOnce(bound) = &selected else {
        return Err("schedule transaction: characterization selected a non-one-shot arm".into());
    };
    let budget_authority = selected_one_shot_budget(state, &context, &bound.entry_id)?;
    Ok(RederivedSourceAdmissionV1 {
        source_kind: InternalAdmissionSourceKindV1::Scheduled,
        source_sha256: source.source_sha256.clone(),
        context,
        authority_action: AuthorityCommitActionV1::OneShot {
            entry_id: bound.entry_id.clone(),
        },
        effect_envelope: AuthorizedEffectEnvelopeV1 {
            allowed_effects: binding.allowed_effects.clone(),
            caps: source.caps.clone(),
        },
        budget_authority,
        selected_at_ms: environment.now_ms,
        terminal_deadline_ms: environment.terminal_deadline_ms,
        authority_snapshot_sha256: String::new(),
    })
}

fn rederive_claimed_support_characterization_source_against_state(
    state: &AuthorityStateModelV1,
    foundation_root: &Path,
    source: &ClaimedSupportCharacterizationSourceV1,
    freshness_bucket: String,
    environment: &AuthorityEnvironmentV1,
    authorization_id: &str,
    request: &CharacterizationAdmissionRequestV1,
) -> Result<RederivedSourceAdmissionV1, BoxError> {
    let foundation = load_schedule_foundation(foundation_root)?;
    let context = rederive_claimed_support_ledger_context_from_foundation(
        &foundation,
        source,
        freshness_bucket,
    )?;
    let binding = foundation
        .claimed_support_profiles
        .get(&source.source.row_id)
        .ok_or("schedule transaction: claimed-support row disappeared")?;
    if request.source != source.source
        || request.characterization_profile_sha256
            != context.identities.characterization_profile.sha256
        || request.characterization_execution_sha256
            != context.identities.case_execution.fingerprint.sha256
        || request.provider_family != context.provider_family
        || request.allowed_effects != binding.allowed_effects
        || request.caps != source.caps
    {
        return Err("schedule transaction: claimed-support request bindings diverged".into());
    }
    let selected =
        select_characterization_authority(state, authorization_id, environment, request)?;
    if selected != source.authority
        || selected != context.identities.admission_attempt.input.authority
    {
        return Err("schedule transaction: claimed-support source carries stale authority".into());
    }
    let AdmissionAuthorityV1::CharacterizationOnce(bound) = &selected else {
        return Err("schedule transaction: claimed-support selected a non-one-shot arm".into());
    };
    let budget_authority = selected_one_shot_budget(state, &context, &bound.entry_id)?;
    Ok(RederivedSourceAdmissionV1 {
        source_kind: InternalAdmissionSourceKindV1::ClaimedSupportCharacterization,
        source_sha256: source.source_sha256.clone(),
        context,
        authority_action: AuthorityCommitActionV1::OneShot {
            entry_id: bound.entry_id.clone(),
        },
        effect_envelope: AuthorizedEffectEnvelopeV1 {
            allowed_effects: binding.allowed_effects.clone(),
            caps: source.caps.clone(),
        },
        budget_authority,
        selected_at_ms: environment.now_ms,
        terminal_deadline_ms: environment.terminal_deadline_ms,
        authority_snapshot_sha256: String::new(),
    })
}

fn rederive_manual_source_against_state(
    state: &AuthorityStateModelV1,
    manual: SealedManualAdmissionV1,
    case_execution_input: CaseExecutionFingerprintInputV1,
    trigger: AdmissionTriggerIdentityV1,
    environment: &AuthorityEnvironmentV1,
    accounting_grant_id: &str,
) -> Result<RederivedSourceAdmissionV1, BoxError> {
    if manual.record.operator != environment.operator
        || manual.record.environment_owner != environment.environment_owner
        || manual.record.scheduler_binary_sha256 != environment.scheduler_binary_sha256
        || manual.record.issued_at_ms > environment.now_ms
        || environment.terminal_deadline_ms > manual.record.expires_at_ms
    {
        return Err(
            "schedule transaction: manual admission environment or deadline diverged".into(),
        );
    }
    let grant = select_manual_accounting_grant(state, accounting_grant_id, environment)?;
    let context = rederive_manual_ledger_context(&manual, case_execution_input, trigger)?;
    let effect_envelope = AuthorizedEffectEnvelopeV1 {
        allowed_effects: manual.record.allowed_effects.clone(),
        caps: manual.record.caps.clone(),
    };
    Ok(RederivedSourceAdmissionV1 {
        source_kind: InternalAdmissionSourceKindV1::GenericManual,
        source_sha256: manual.record.input_source_sha256.clone(),
        context,
        authority_action: AuthorityCommitActionV1::Manual {
            admission: Box::new(manual.clone()),
        },
        effect_envelope,
        budget_authority: LedgerBudgetAuthorityV1::ManualUnallocated {
            manual_admission_sha256: manual_admission_sha256(&manual.record)?,
            accounting_grant_sha256: grant.grant_sha256.clone(),
            budgets: grant.budgets.clone(),
        },
        selected_at_ms: environment.now_ms,
        terminal_deadline_ms: environment.terminal_deadline_ms,
        authority_snapshot_sha256: String::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_source_proposal_for_capability<C, I, F>(
    capability: &C,
    selected: RederivedSourceAdmissionV1,
    supervisor: Option<PreparedSupervisorV1>,
    trusted_root: PlannedDirectoryBindingV1,
    requested_cwd: PlannedDirectoryBindingV1,
    recorded_at_ms: i64,
    initial_checks: &mut I,
    final_checks: &mut F,
) -> Result<AdmissionCommitProposalV1, BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
    I: ZeroEffectPreflightChecks,
    F: ZeroEffectPreflightChecks,
{
    if selected.selected_at_ms != recorded_at_ms
        || !local_file::valid_sha256(&selected.authority_snapshot_sha256)
    {
        return Err("schedule transaction: source selection is stale or unbound at commit".into());
    }
    let authority = FileAuthorityJournal::open_existing(capability)?;
    if authority.snapshot_sha256 != selected.authority_snapshot_sha256 {
        return Err("schedule transaction: authority changed after source selection".into());
    }
    let admission = FileAdmissionJournal::open(capability)?;
    let mut equivalent_work_preview = admission.journal.state().equivalent_work.clone();
    let preview = equivalent_work_preview.reserve_or_reuse(
        &selected.context.identities,
        selected
            .context
            .identities
            .admission_attempt
            .input
            .authority
            .clone(),
        recorded_at_ms,
    )?;
    let (ledger, supervisor) = match preview {
        EquivalentWorkDecisionV1::Reserved(_) => {
            let supervisor = supervisor.ok_or(
                "schedule transaction: newly reserved work requires a prepared supervisor",
            )?;
            let ledger = FileCompatibilityLedger::open(capability)?;
            let reservation = ledger.prepare_reservation(
                &LedgerReservationRequestV1::from_derived_context(
                    &selected.context,
                    &selected.budget_authority,
                ),
                recorded_at_ms,
            )?;
            (Some(reservation), Some(supervisor))
        }
        EquivalentWorkDecisionV1::Reused(_) => (None, None),
    };
    let preflight_binding = admission_preflight_binding(
        selected.source_kind,
        &selected.source_sha256,
        &selected.context,
        &selected.authority_action,
        &selected.effect_envelope,
        &selected.authority_snapshot_sha256,
        ledger.as_ref(),
        supervisor.as_ref().map(PreparedSupervisorV1::record),
        supervisor
            .as_ref()
            .map(PreparedSupervisorV1::deadline)
            .map(HardDeadline::record),
        &trusted_root,
        &requested_cwd,
        selected.terminal_deadline_ms,
        recorded_at_ms,
    )?;
    let initial_preflight = run_zero_effect_preflight(
        PreflightFenceV1::Initial,
        preflight_binding.clone(),
        initial_checks,
    )
    .map_err(|refusal| {
        format!(
            "schedule transaction: initial preflight refused: {}",
            refusal.code
        )
    })?;
    let final_preflight =
        run_zero_effect_preflight(PreflightFenceV1::Final, preflight_binding, final_checks)
            .map_err(|refusal| {
                format!(
                    "schedule transaction: final preflight refused: {}",
                    refusal.code
                )
            })?;
    Ok(AdmissionCommitProposalV1 {
        source_kind: selected.source_kind,
        source_sha256: selected.source_sha256,
        authority_snapshot_sha256: selected.authority_snapshot_sha256,
        context: selected.context,
        authority_action: selected.authority_action,
        effect_envelope: selected.effect_envelope,
        ledger,
        supervisor,
        initial_preflight,
        final_preflight,
        trusted_root,
        requested_cwd,
        terminal_deadline_ms: selected.terminal_deadline_ms,
        recorded_at_ms,
    })
}

pub(super) struct AdmissionTransactionSessionV1<'a, C: ?Sized> {
    capability: &'a C,
    authority_state: AuthorityStateModelV1,
    authority_snapshot_sha256: String,
}

pub(super) fn begin_admission_transaction<'a, C>(
    capability: &'a C,
) -> Result<AdmissionTransactionSessionV1<'a, C>, BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
{
    recover_committed_state(capability)?;
    let authority = FileAuthorityJournal::open_existing(capability)?;
    Ok(AdmissionTransactionSessionV1 {
        capability,
        authority_state: authority.snapshot.state,
        authority_snapshot_sha256: authority.snapshot_sha256,
    })
}

impl<C> AdmissionTransactionSessionV1<'_, C>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
{
    pub(super) fn rederive_scheduled_standing_source(
        &self,
        foundation_root: &Path,
        source: &ScheduledExecutionSourceV1,
        freshness_bucket: String,
        environment: &AuthorityEnvironmentV1,
        grant_id: &str,
        request: &StandingAdmissionRequestV1,
    ) -> Result<RederivedSourceAdmissionV1, BoxError> {
        rederive_scheduled_standing_source_against_state(
            &self.authority_state,
            foundation_root,
            source,
            freshness_bucket,
            environment,
            grant_id,
            request,
        )?
        .bind_authority_snapshot(&self.authority_snapshot_sha256)
    }

    pub(super) fn rederive_scheduled_characterization_source(
        &self,
        foundation_root: &Path,
        source: &ScheduledExecutionSourceV1,
        freshness_bucket: String,
        environment: &AuthorityEnvironmentV1,
        authorization_id: &str,
        request: &CharacterizationAdmissionRequestV1,
    ) -> Result<RederivedSourceAdmissionV1, BoxError> {
        rederive_scheduled_characterization_source_against_state(
            &self.authority_state,
            foundation_root,
            source,
            freshness_bucket,
            environment,
            authorization_id,
            request,
        )?
        .bind_authority_snapshot(&self.authority_snapshot_sha256)
    }

    pub(super) fn rederive_claimed_support_characterization_source(
        &self,
        foundation_root: &Path,
        source: &ClaimedSupportCharacterizationSourceV1,
        freshness_bucket: String,
        environment: &AuthorityEnvironmentV1,
        authorization_id: &str,
        request: &CharacterizationAdmissionRequestV1,
    ) -> Result<RederivedSourceAdmissionV1, BoxError> {
        rederive_claimed_support_characterization_source_against_state(
            &self.authority_state,
            foundation_root,
            source,
            freshness_bucket,
            environment,
            authorization_id,
            request,
        )?
        .bind_authority_snapshot(&self.authority_snapshot_sha256)
    }

    pub(super) fn rederive_manual_source(
        &self,
        manual: SealedManualAdmissionV1,
        case_execution_input: CaseExecutionFingerprintInputV1,
        trigger: AdmissionTriggerIdentityV1,
        environment: &AuthorityEnvironmentV1,
        accounting_grant_id: &str,
    ) -> Result<RederivedSourceAdmissionV1, BoxError> {
        rederive_manual_source_against_state(
            &self.authority_state,
            manual,
            case_execution_input,
            trigger,
            environment,
            accounting_grant_id,
        )?
        .bind_authority_snapshot(&self.authority_snapshot_sha256)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn admit<I, F>(
        &self,
        selected: RederivedSourceAdmissionV1,
        supervisor: Option<PreparedSupervisorV1>,
        trusted_root: PlannedDirectoryBindingV1,
        requested_cwd: PlannedDirectoryBindingV1,
        recorded_at_ms: i64,
        initial_checks: &mut I,
        final_checks: &mut F,
    ) -> Result<PublishedAdmissionV1, BoxError>
    where
        I: ZeroEffectPreflightChecks,
        F: ZeroEffectPreflightChecks,
    {
        let proposal = prepare_source_proposal_for_capability(
            self.capability,
            selected,
            supervisor,
            trusted_root,
            requested_cwd,
            recorded_at_ms,
            initial_checks,
            final_checks,
        )?;
        commit_prevalidated_proposal_for_capability(self.capability, proposal)
    }
}

pub(super) fn build_admission_commit(
    admission: &FileAdmissionJournal<'_>,
    authority: &AuthorityJournalOpen<'_>,
    proposal: AdmissionCommitProposalV1,
) -> Result<(AdmissionCommitV1, Option<HardDeadline>), BoxError> {
    proposal.context.identities.validate()?;
    if proposal.recorded_at_ms <= 0
        || !local_file::valid_sha256(&proposal.source_sha256)
        || proposal.authority_snapshot_sha256 != authority.snapshot_sha256
    {
        return Err("schedule transaction: proposal time/source binding is invalid".into());
    }
    let expected_preflight_binding = admission_preflight_binding(
        proposal.source_kind,
        &proposal.source_sha256,
        &proposal.context,
        &proposal.authority_action,
        &proposal.effect_envelope,
        &proposal.authority_snapshot_sha256,
        proposal.ledger.as_ref(),
        proposal
            .supervisor
            .as_ref()
            .map(PreparedSupervisorV1::record),
        proposal
            .supervisor
            .as_ref()
            .map(PreparedSupervisorV1::deadline)
            .map(HardDeadline::record),
        &proposal.trusted_root,
        &proposal.requested_cwd,
        proposal.terminal_deadline_ms,
        proposal.recorded_at_ms,
    )?;
    if proposal.initial_preflight.binding != expected_preflight_binding
        || proposal.final_preflight.binding != expected_preflight_binding
    {
        return Err("schedule transaction: preflight result belongs to another admission".into());
    }
    let mut admission_state_after = admission.state().clone();
    let decision = admission_state_after.equivalent_work.reserve_or_reuse(
        &proposal.context.identities,
        proposal
            .context
            .identities
            .admission_attempt
            .input
            .authority
            .clone(),
        proposal.recorded_at_ms,
    )?;
    admission_state_after.validate()?;
    let (disposition, hard_deadline) = match (decision, proposal.ledger, proposal.supervisor) {
        (EquivalentWorkDecisionV1::Reserved(equivalent_work), Some(ledger), Some(supervisor)) => {
            let (supervisor, deadline) = supervisor.into_parts();
            validate_prepared_reservation_context(&ledger, &proposal.context)?;
            validate_admission_deadline(
                &proposal.context,
                &ledger,
                &supervisor,
                &deadline,
                proposal.terminal_deadline_ms,
                proposal.recorded_at_ms,
            )?;
            (
                AdmissionDispositionV1::Reserved {
                    equivalent_work,
                    ledger_sha256: prepared_reservation_sha256(&ledger)?,
                    deadline_derivation: Box::new(deadline.record().clone()),
                    supervisor_sha256: supervisor_record_sha256(&supervisor)?,
                    ledger: Box::new(ledger),
                    supervisor: Box::new(supervisor),
                },
                Some(deadline),
            )
        }
        (EquivalentWorkDecisionV1::Reused(consumption), None, None) => {
            (AdmissionDispositionV1::Reused { consumption }, None)
        }
        _ => return Err(
            "schedule transaction: reserved work requires ledger/supervisor and reuse forbids them"
                .into(),
        ),
    };
    let admission_state_before_sha256 = admission_state_sha256(admission.state())?;
    let admission_state_after_sha256 = admission_state_sha256(&admission_state_after)?;
    let initial_preflight_sha256 = preflight_pass_sha256(&proposal.initial_preflight)?;
    let final_preflight_sha256 = preflight_pass_sha256(&proposal.final_preflight)?;
    let commit_id = format!(
        "admission-{}",
        proposal.context.identities.attempt_idempotency_key
    );
    let mut commit = AdmissionCommitV1 {
        schema_version: 1,
        generation: admission.next_generation(),
        previous_commit: admission.previous_record(),
        commit_id,
        commit_identity_sha256: "0".repeat(64),
        source_kind: proposal.source_kind,
        source_sha256: proposal.source_sha256,
        context: proposal.context,
        authority_action: proposal.authority_action,
        effect_envelope: proposal.effect_envelope,
        authority_before_snapshot_sha256: authority.snapshot_sha256.clone(),
        authority_after_snapshot: authority.snapshot.clone(),
        authority_after_snapshot_sha256: authority.snapshot_sha256.clone(),
        admission_state_before_sha256,
        admission_state_after,
        disposition,
        initial_preflight: proposal.initial_preflight,
        final_preflight: proposal.final_preflight,
        trusted_root: proposal.trusted_root,
        requested_cwd: proposal.requested_cwd,
        terminal_deadline_ms: proposal.terminal_deadline_ms,
        recorded_at_ms: proposal.recorded_at_ms,
    };
    commit.commit_identity_sha256 = commit_identity_sha256(
        &commit,
        &admission_state_after_sha256,
        &initial_preflight_sha256,
        &final_preflight_sha256,
    )?;

    let mut authority_state_after = authority.snapshot.state.clone();
    match &commit.authority_action {
        AuthorityCommitActionV1::Standing => {
            if !matches!(
                commit.context.identities.admission_attempt.input.authority,
                AdmissionAuthorityV1::StandingGrant(_)
            ) {
                return Err(
                    "schedule transaction: standing action has non-standing authority".into(),
                );
            }
        }
        AuthorityCommitActionV1::OneShot { entry_id } => {
            authority_state_after.consume_one_shot(
                entry_id,
                &commit.commit_identity_sha256,
                commit.recorded_at_ms,
            )?;
        }
        AuthorityCommitActionV1::Manual { admission } => {
            authority_state_after.consume_manual_admission(
                admission.as_ref().clone(),
                &commit.commit_identity_sha256,
                commit.recorded_at_ms,
            )?;
        }
    }
    let (authority_after_snapshot, authority_after_snapshot_sha256) = authority
        .journal
        .preview_append(&authority_state_after, commit.recorded_at_ms)?;
    commit.authority_after_snapshot = authority_after_snapshot;
    commit.authority_after_snapshot_sha256 = authority_after_snapshot_sha256;
    validate_commit_against_state(&commit, admission.state())?;
    Ok((commit, hard_deadline))
}

fn publish_authority_commit<C>(capability: &C, commit: &AdmissionCommitV1) -> Result<(), BoxError>
where
    C: AuthorityStateCapability + ?Sized,
{
    let mut current = FileAuthorityJournal::open_existing(capability)?;
    if current.snapshot_sha256 == commit.authority_before_snapshot_sha256 {
        let (snapshot, sha256) = current.journal.append(
            &commit.authority_after_snapshot.state,
            commit.authority_after_snapshot.recorded_at_ms,
        )?;
        if snapshot != commit.authority_after_snapshot
            || sha256 != commit.authority_after_snapshot_sha256
        {
            return Err("schedule transaction: published authority preview diverged".into());
        }
        current.snapshot = snapshot;
        current.snapshot_sha256 = sha256;
    } else if !current.journal.contains_exact_snapshot(
        &commit.authority_after_snapshot,
        &commit.authority_after_snapshot_sha256,
    )? {
        return Err("schedule transaction: authority history diverged before publication".into());
    }
    commit.authority_action.validate_binding(
        &commit.context,
        &commit.commit_identity_sha256,
        commit.recorded_at_ms,
        &current.snapshot,
    )
}

fn publish_committed_state<C>(
    capability: &C,
    commit: &AdmissionCommitV1,
) -> Result<Option<SupervisorRecordV1>, BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
{
    publish_authority_commit(capability, commit)?;
    match &commit.disposition {
        AdmissionDispositionV1::Reserved {
            ledger, supervisor, ..
        } => {
            let mut opened_ledger = FileCompatibilityLedger::open(capability)?;
            let (_outcome, published) =
                opened_ledger.commit_prepared_reservation(ledger.as_ref().clone())?;
            if &published != ledger.as_ref() {
                return Err("schedule transaction: published ledger reservation diverged".into());
            }
            let supervisor_directory = capability.supervisor_directory().canonical_path();
            let (latest, _latest_sha256) =
                ensure_prepared_supervisor(&supervisor_directory, supervisor)?;
            Ok(Some(latest))
        }
        AdmissionDispositionV1::Reused { .. } => Ok(None),
    }
}

fn ledger_reconciliation(terminal: &AdmissionTerminalV1) -> ReconciliationDecisionV1 {
    match &terminal.disposition {
        AdmissionTerminalDispositionV1::ProvedPreEffect { evidence_sha256 } => {
            ReconciliationDecisionV1::ProvedPreEffect {
                evidence_sha256: evidence_sha256.clone(),
                reconciled_at_ms: terminal.recorded_at_ms,
            }
        }
        AdmissionTerminalDispositionV1::ValidTerminal {
            evidence,
            usage,
            prompt_was_accepted,
        } => ReconciliationDecisionV1::ValidTerminal {
            evidence_sha256: evidence.evidence_sha256.clone(),
            usage: usage.clone(),
            prompt_was_accepted: *prompt_was_accepted,
            reconciled_at_ms: terminal.recorded_at_ms,
        },
        AdmissionTerminalDispositionV1::Conservative {
            evidence_sha256,
            reason,
            prompt_may_have_been_accepted,
        } => ReconciliationDecisionV1::Conservative {
            evidence_sha256: evidence_sha256.clone(),
            reason: *reason,
            prompt_may_have_been_accepted: *prompt_may_have_been_accepted,
            reconciled_at_ms: terminal.recorded_at_ms,
        },
    }
}

fn publish_terminal_state<C>(
    capability: &C,
    commit: &AdmissionCommitV1,
    terminal: &AdmissionTerminalV1,
    terminal_sha256: &str,
) -> Result<(), BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
{
    let AdmissionDispositionV1::Reserved {
        ledger,
        supervisor: prepared,
        ..
    } = &commit.disposition
    else {
        return Err("schedule transaction: reused commit cannot publish a terminal".into());
    };
    let supervisor_directory = capability.supervisor_directory().canonical_path();
    let (latest, latest_sha256) = ensure_prepared_supervisor(&supervisor_directory, prepared)?;
    if latest != terminal.supervisor || latest_sha256 != terminal.supervisor_journal_sha256 {
        return Err("schedule transaction: terminal supervisor tail diverged".into());
    }
    let mut opened_ledger = FileCompatibilityLedger::open(capability)?;
    opened_ledger.reconcile(&ledger.reservation_id, ledger_reconciliation(terminal))?;

    let AuthorityCommitActionV1::OneShot { entry_id } = &commit.authority_action else {
        return Ok(());
    };
    let mut authority = FileAuthorityJournal::open_existing(capability)?;
    let phase = &authority
        .snapshot
        .state
        .one_shots
        .get(entry_id)
        .ok_or("schedule transaction: terminal one-shot lifecycle is absent")?
        .phase;
    match phase {
        OneShotLifecyclePhaseV1::ConsumedUnreconciled {
            admission_commit_sha256,
            ..
        } if admission_commit_sha256 == &commit.commit_identity_sha256 => {
            let mut next = authority.snapshot.state.clone();
            next.reconcile_one_shot(
                entry_id,
                &commit.commit_identity_sha256,
                terminal_sha256,
                terminal.recorded_at_ms,
            )?;
            authority.journal.append(&next, terminal.recorded_at_ms)?;
            Ok(())
        }
        OneShotLifecyclePhaseV1::Reconciled {
            admission_commit_sha256,
            terminal_record_sha256,
            ..
        } if admission_commit_sha256 == &commit.commit_identity_sha256
            && terminal_record_sha256 == terminal_sha256 =>
        {
            Ok(())
        }
        _ => Err("schedule transaction: one-shot terminal reconciliation diverged".into()),
    }
}

fn terminal_optional_text(value: Option<&str>) -> OptionalTextV1 {
    match value {
        Some(value) => OptionalTextV1::Text {
            value: value.to_owned(),
        },
        None => OptionalTextV1::Absent,
    }
}

fn terminal_identity(value: (&str, Option<&str>, Option<&str>)) -> EffectiveIdentityV1 {
    EffectiveIdentityV1 {
        model: value.0.to_owned(),
        effort: terminal_optional_text(value.1),
        mode: terminal_optional_text(value.2),
    }
}

fn durable_terminal_disposition(
    commit: &AdmissionCommitV1,
    supervisor: &SupervisorRecordV1,
    proof: AdmissionTerminalProofV1,
    recorded_at_ms: i64,
) -> Result<AdmissionTerminalDispositionV1, BoxError> {
    match proof {
        AdmissionTerminalProofV1::ProvedPreEffect { evidence_sha256 } => {
            Ok(AdmissionTerminalDispositionV1::ProvedPreEffect { evidence_sha256 })
        }
        AdmissionTerminalProofV1::Conservative {
            evidence_sha256,
            reason,
            prompt_may_have_been_accepted,
        } => Ok(AdmissionTerminalDispositionV1::Conservative {
            evidence_sha256,
            reason,
            prompt_may_have_been_accepted,
        }),
        AdmissionTerminalProofV1::ValidTerminal { child } => {
            let AdmissionDispositionV1::Reserved {
                equivalent_work,
                ledger,
                ..
            } = &commit.disposition
            else {
                return Err("schedule transaction: reused admission has no terminal proof".into());
            };
            let OptionalChildArtifactRefV1::Artifact {
                value: supervisor_child,
            } = &supervisor.child_artifact
            else {
                return Err(
                    "schedule transaction: valid terminal has no supervisor child artifact".into(),
                );
            };
            if supervisor_child != child.child_reference() {
                return Err(
                    "schedule transaction: terminal proof does not match the supervisor child"
                        .into(),
                );
            }
            let aggregate_sha256 = match &supervisor_child.aggregate_sha256 {
                OptionalSha256V1::Sha256 { value } => value.clone(),
                OptionalSha256V1::Absent => {
                    return Err(
                        "schedule transaction: valid terminal has no joined child aggregate".into(),
                    )
                }
            };
            let aggregate = child.aggregate();
            let execution = &commit.context.identities.case_execution.input;
            let requested_identity = terminal_identity(aggregate.requested_identity());
            let observed_identity = terminal_identity(aggregate.observed_identity());
            if aggregate.case_id() != commit.context.case_id
                || aggregate.candidate_sha256() != execution.candidate.sha256
                || aggregate.candidate_length_bytes() != execution.candidate.length_bytes
                || aggregate.manifest_sha256() != execution.bindings.run_manifest_sha256
                || requested_identity != execution.requested_identity
                || observed_identity != execution.expected_effective_identity
                || aggregate.terminal_at_ms() > recorded_at_ms
                || !aggregate.prompt_was_accepted()
            {
                return Err(
                    "schedule transaction: immutable child aggregate identity diverged".into(),
                );
            }
            let usage = UsageChargeV1 {
                attempts: 1,
                tokens: aggregate
                    .observed_tokens()
                    .unwrap_or(ledger.caps.max_tokens),
                cost_microusd: aggregate
                    .observed_cost_microusd()
                    .unwrap_or(ledger.caps.max_cost_microusd),
                elapsed_millis: aggregate.elapsed_millis(),
            };
            if !usage_within_caps(&usage, &ledger.caps) {
                return Err(
                    "schedule transaction: immutable child aggregate usage exceeds caps".into(),
                );
            }
            let reusable = !matches!(
                equivalent_work.evidence_purpose,
                crate::compatibility_schedule::EvidencePurposeV1::Characterization
                    | crate::compatibility_schedule::EvidencePurposeV1::ManualDiagnostic
            );
            let evidence = CompletedEquivalentEvidenceV1 {
                reservation_id: equivalent_work.reservation_id.clone(),
                evidence_sha256: aggregate_sha256,
                satisfied_purpose: equivalent_work.evidence_purpose,
                freshness_bucket: equivalent_work.freshness_bucket.clone(),
                characterization_profile: equivalent_work.characterization_profile.clone(),
                case_execution: equivalent_work.case_execution.clone(),
                expected_effective_identity: execution.expected_effective_identity.clone(),
                observed_effective_identity: observed_identity,
                provenance: ConsumptionEvidenceProvenanceV1::Ordinary,
                reusable,
                terminal_at_ms: aggregate.terminal_at_ms(),
            };
            Ok(AdmissionTerminalDispositionV1::ValidTerminal {
                evidence: Box::new(evidence),
                usage,
                prompt_was_accepted: true,
            })
        }
    }
}

fn build_admission_terminal(
    commit: &AdmissionCommitV1,
    before: &AdmissionStateV1,
    supervisor: SupervisorRecordV1,
    supervisor_journal_sha256: String,
    disposition: AdmissionTerminalDispositionV1,
    recorded_at_ms: i64,
) -> Result<AdmissionTerminalV1, BoxError> {
    let AdmissionDispositionV1::Reserved {
        equivalent_work, ..
    } = &commit.disposition
    else {
        return Err("schedule transaction: reused commit cannot be terminalized".into());
    };
    let mut after = before.clone();
    match &disposition {
        AdmissionTerminalDispositionV1::ProvedPreEffect { evidence_sha256 } => {
            after.equivalent_work.record_completed(pre_effect_evidence(
                commit,
                equivalent_work,
                evidence_sha256.clone(),
                recorded_at_ms,
            ))?;
        }
        AdmissionTerminalDispositionV1::ValidTerminal { evidence, .. } => {
            after
                .equivalent_work
                .record_completed(evidence.as_ref().clone())?;
        }
        AdmissionTerminalDispositionV1::Conservative { .. } => {}
    }
    let value = AdmissionTerminalV1 {
        schema_version: 1,
        generation: commit.generation,
        terminal_id: format!("terminal-{}", commit.commit_identity_sha256),
        admission_commit_identity_sha256: commit.commit_identity_sha256.clone(),
        supervisor,
        supervisor_journal_sha256,
        disposition,
        admission_state_before_sha256: admission_state_sha256(before)?,
        admission_state_after: after,
        recorded_at_ms,
    };
    validate_terminal_against_state(&value, commit, before)?;
    Ok(value)
}

pub(super) fn reconcile_pending_admission<C>(
    capability: &C,
    proof: AdmissionTerminalProofV1,
    recorded_at_ms: i64,
) -> Result<AdmissionTerminalV1, BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
{
    recover_committed_state(capability)?;
    let mut admission = FileAdmissionJournal::open(capability)?;
    let pending = admission.journal.pending_reserved.clone();
    let existing = admission.terminals.last().map(|(value, _)| value.clone());
    let commit = match (&pending, &existing) {
        (Some(commit), _) => commit.clone(),
        (None, Some(existing)) => admission
            .commits
            .iter()
            .find(|(commit, _)| {
                commit.commit_identity_sha256 == existing.admission_commit_identity_sha256
            })
            .map(|(commit, _)| commit.clone())
            .ok_or("schedule transaction: terminal admission commit is absent")?,
        (None, None) => {
            return Err("schedule transaction: no pending admission matches reconciliation".into())
        }
    };
    let AdmissionDispositionV1::Reserved { supervisor, .. } = &commit.disposition else {
        return Err("schedule transaction: reused admission cannot be reconciled".into());
    };
    let supervisor_directory = capability.supervisor_directory().canonical_path();
    let (latest, latest_sha256) = ensure_prepared_supervisor(&supervisor_directory, supervisor)?;
    let disposition = durable_terminal_disposition(&commit, &latest, proof, recorded_at_ms)?;
    if pending.is_none() {
        let existing = existing.expect("the no-pending branch selected an existing terminal");
        return if existing.disposition == disposition && existing.recorded_at_ms == recorded_at_ms {
            Ok(existing)
        } else {
            Err("schedule transaction: repeated reconciliation proof diverged".into())
        };
    }
    let terminal = build_admission_terminal(
        &commit,
        admission.journal.state(),
        latest,
        latest_sha256,
        disposition,
        recorded_at_ms,
    )?;
    let terminal_sha256 = admission.journal.append_terminal(terminal.clone())?;
    publish_terminal_state(capability, &commit, &terminal, &terminal_sha256)?;
    Ok(terminal)
}

/// Completes every durable projection implied by a valid admission commit. It deliberately has no
/// runner handoff argument: recovery may make authority, ledger, and Prepared supervisor state
/// visible, but it can never repeat a possibly accepted provider effect.
pub(super) fn recover_committed_state<C>(capability: &C) -> Result<usize, BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
{
    let admission = FileAdmissionJournal::open(capability)?;
    for (commit, _sha256) in &admission.commits {
        publish_committed_state(capability, commit)?;
    }
    let commits = admission
        .commits
        .iter()
        .map(|(commit, _)| (commit.generation, commit))
        .collect::<BTreeMap<_, _>>();
    for (terminal, terminal_sha256) in &admission.terminals {
        let commit = commits
            .get(&terminal.generation)
            .ok_or("schedule transaction: terminal recovery lost its commit")?;
        publish_terminal_state(capability, commit, terminal, terminal_sha256)?;
    }
    Ok(admission.commits.len())
}

pub(super) struct AdmittedRunCapabilityV1 {
    commit_identity_sha256: String,
    context: DerivedLedgerAdmissionContextV1,
    effect_envelope: AuthorizedEffectEnvelopeV1,
    supervisor_record_id: String,
    action_directories: PinnedActionDirectoriesV1,
    hard_deadline: HardDeadline,
}

impl AdmittedRunCapabilityV1 {
    pub(super) fn commit_identity_sha256(&self) -> &str {
        &self.commit_identity_sha256
    }

    pub(super) fn context(&self) -> &DerivedLedgerAdmissionContextV1 {
        &self.context
    }

    pub(super) fn supervisor_record_id(&self) -> &str {
        &self.supervisor_record_id
    }

    pub(super) fn effect_envelope(&self) -> &AuthorizedEffectEnvelopeV1 {
        &self.effect_envelope
    }

    pub(super) fn action_directories(&self) -> &PinnedActionDirectoriesV1 {
        &self.action_directories
    }

    pub(super) fn hard_deadline(&self) -> &HardDeadline {
        &self.hard_deadline
    }
}

pub(super) enum PublishedAdmissionV1 {
    Admitted(Box<AdmittedRunCapabilityV1>),
    Reused(Box<ConsumptionRecordV1>),
}

/// The caller must derive and reselect the source, authority, and budget policy while retaining the
/// same owner-wide plus authority-state capability. This lower-level function then owns the only
/// durable admission commit and projection order.
fn commit_prevalidated_proposal_for_capability<C>(
    capability: &C,
    proposal: AdmissionCommitProposalV1,
) -> Result<PublishedAdmissionV1, BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
{
    let authority = FileAuthorityJournal::open_existing(capability)?;
    let mut admission = FileAdmissionJournal::open(capability)?;
    let action_directories =
        pin_action_directories(&proposal.trusted_root, &proposal.requested_cwd)?;
    let (commit, hard_deadline) = build_admission_commit(&admission.journal, &authority, proposal)?;
    admission.journal.append(commit.clone())?;
    drop(authority);
    let published_supervisor = publish_committed_state(capability, &commit)?;
    if !action_directories.trusted_root.current_path_matches()
        || !action_directories.requested_cwd.current_path_matches()
    {
        return Err("schedule transaction: action directories changed after commit".into());
    }
    match (&commit.disposition, published_supervisor) {
        (AdmissionDispositionV1::Reserved { supervisor, .. }, Some(latest_supervisor))
            if &latest_supervisor == supervisor.as_ref() =>
        {
            Ok(PublishedAdmissionV1::Admitted(Box::new(
                AdmittedRunCapabilityV1 {
                    commit_identity_sha256: commit.commit_identity_sha256,
                    context: commit.context,
                    effect_envelope: commit.effect_envelope,
                    supervisor_record_id: supervisor.supervisor_record_id.clone(),
                    action_directories,
                    hard_deadline: hard_deadline.ok_or(
                        "schedule transaction: admitted reservation lost its hard deadline",
                    )?,
                },
            )))
        }
        (AdmissionDispositionV1::Reused { consumption }, None) => {
            Ok(PublishedAdmissionV1::Reused(Box::new(consumption.clone())))
        }
        _ => Err("schedule transaction: publication did not stop at the expected phase".into()),
    }
}

pub(super) trait AdmittedRunnerHandoff {
    type Output;

    fn handoff(&mut self, capability: AdmittedRunCapabilityV1) -> Result<Self::Output, BoxError>;
}

pub(super) fn handoff_admitted<R: AdmittedRunnerHandoff>(
    capability: AdmittedRunCapabilityV1,
    runner: &mut R,
) -> Result<R::Output, BoxError> {
    runner.handoff(capability)
}

#[derive(Clone, Debug)]
pub(super) struct AdmissionJournalOpen<'lock> {
    pub(super) journal: FileAdmissionJournal<'lock>,
    pub(super) state: AdmissionStateV1,
    pub(super) commits: Vec<(AdmissionCommitV1, String)>,
    pub(super) terminals: Vec<(AdmissionTerminalV1, String)>,
}

#[derive(Clone, Debug)]
pub(super) struct FileAdmissionJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    next_generation: u64,
    previous_sha256: Option<String>,
    state: AdmissionStateV1,
    pending_reserved: Option<AdmissionCommitV1>,
}

impl<'lock> FileAdmissionJournal<'lock> {
    fn generation_name(generation: u64) -> String {
        format!("{COMMIT_PREFIX}{generation:020}.json")
    }

    fn terminal_name(generation: u64) -> String {
        format!("{TERMINAL_PREFIX}{generation:020}.json")
    }

    fn entries(directory: &local_file::PinnedDirectory) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule transaction: retained admission directory changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "schedule transaction: non-UTF8 admission entry")?;
            if !name.starts_with(COMMIT_PREFIX) {
                continue;
            }
            let raw = name
                .strip_prefix(COMMIT_PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
                .ok_or("schedule transaction: malformed admission generation name")?;
            if raw.len() != 20 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("schedule transaction: malformed admission generation number".into());
            }
            entries.push((raw.parse::<u64>()?, name));
        }
        if entries.len() > MAX_COMMIT_GENERATIONS || !directory.current_path_matches() {
            return Err(
                "schedule transaction: admission generation scan is unbounded or unstable".into(),
            );
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    fn terminal_entries(
        directory: &local_file::PinnedDirectory,
    ) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule transaction: retained admission directory changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "schedule transaction: non-UTF8 terminal entry")?;
            if !name.starts_with(TERMINAL_PREFIX) {
                continue;
            }
            let raw = name
                .strip_prefix(TERMINAL_PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
                .ok_or("schedule transaction: malformed terminal generation name")?;
            if raw.len() != 20 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("schedule transaction: malformed terminal generation number".into());
            }
            entries.push((raw.parse::<u64>()?, name));
        }
        if entries.len() > MAX_COMMIT_GENERATIONS || !directory.current_path_matches() {
            return Err("schedule transaction: terminal scan is unbounded or unstable".into());
        }
        entries.sort_by_key(|(generation, _)| *generation);
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err("schedule transaction: duplicate terminal generation".into());
        }
        Ok(entries)
    }

    fn read_commit(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<(AdmissionCommitV1, String), BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let file = directory.open_regular_file(OsStr::new(name), "admission commit")?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > MAX_COMMIT_BYTES
        {
            return Err(
                "schedule transaction: admission commit is not owner-only mode-0600".into(),
            );
        }
        let read = local_file::read_open_regular_file_bounded(
            &file,
            "admission commit",
            MAX_COMMIT_BYTES,
        )?;
        let value: AdmissionCommitV1 = serde_json::from_slice(&read.bytes)
            .map_err(|error| format!("schedule transaction: invalid admission commit: {error}"))?;
        if canonical_bytes(&value)? != read.bytes {
            return Err("schedule transaction: admission commit is not canonical JSON".into());
        }
        Ok((value, read.sha256))
    }

    fn read_terminal(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<(AdmissionTerminalV1, String), BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let file = directory.open_regular_file(OsStr::new(name), "admission terminal")?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > MAX_COMMIT_BYTES
        {
            return Err("schedule transaction: terminal is not owner-only mode-0600".into());
        }
        let read = local_file::read_open_regular_file_bounded(
            &file,
            "admission terminal",
            MAX_COMMIT_BYTES,
        )?;
        let value: AdmissionTerminalV1 = serde_json::from_slice(&read.bytes)
            .map_err(|error| format!("schedule transaction: invalid terminal: {error}"))?;
        if canonical_bytes(&value)? != read.bytes {
            return Err("schedule transaction: terminal is not canonical JSON".into());
        }
        Ok((value, read.sha256))
    }

    pub(super) fn open<C: AdmissionStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<AdmissionJournalOpen<'lock>, BoxError> {
        let directory = capability.admission_directory();
        let mut state = AdmissionStateV1::new();
        let mut previous_sha256: Option<String> = None;
        let mut commits = Vec::new();
        let mut terminals = Vec::new();
        let mut terminal_entries = Self::terminal_entries(directory)?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut pending_reserved: Option<AdmissionCommitV1> = None;
        for (index, (generation, name)) in Self::entries(directory)?.into_iter().enumerate() {
            if pending_reserved.is_some() {
                return Err(
                    "schedule transaction: a later commit bypassed terminal reconciliation".into(),
                );
            }
            let expected = u64::try_from(index + 1)?;
            if generation != expected {
                return Err(
                    "schedule transaction: admission generations are not contiguous".into(),
                );
            }
            let (commit, sha256) = Self::read_commit(directory, &name)?;
            if commit.generation != generation {
                return Err(
                    "schedule transaction: admission filename/record generation diverged".into(),
                );
            }
            match (&commit.previous_commit, previous_sha256.as_deref()) {
                (OptionalSha256V1::Absent, None) => {}
                (OptionalSha256V1::Sha256 { value }, Some(previous)) if value == previous => {}
                _ => {
                    return Err(
                        "schedule transaction: admission commit hash chain is invalid".into(),
                    )
                }
            }
            validate_commit_against_state(&commit, &state)?;
            state = commit.admission_state_after.clone();
            previous_sha256 = Some(sha256.clone());
            let terminal_name = terminal_entries.remove(&generation);
            match (&commit.disposition, terminal_name) {
                (AdmissionDispositionV1::Reserved { .. }, Some(name)) => {
                    let (terminal, terminal_sha256) = Self::read_terminal(directory, &name)?;
                    validate_terminal_against_state(&terminal, &commit, &state)?;
                    state = terminal.admission_state_after.clone();
                    terminals.push((terminal, terminal_sha256));
                }
                (AdmissionDispositionV1::Reserved { .. }, None) => {
                    pending_reserved = Some(commit.clone());
                }
                (AdmissionDispositionV1::Reused { .. }, Some(_)) => {
                    return Err("schedule transaction: reused commit has a terminal record".into())
                }
                (AdmissionDispositionV1::Reused { .. }, None) => {}
            }
            commits.push((commit, sha256));
        }
        if !terminal_entries.is_empty() {
            return Err("schedule transaction: terminal has no matching admission commit".into());
        }
        state.validate()?;
        Ok(AdmissionJournalOpen {
            journal: Self {
                directory,
                next_generation: u64::try_from(commits.len())?.saturating_add(1),
                previous_sha256,
                state: state.clone(),
                pending_reserved,
            },
            state,
            commits,
            terminals,
        })
    }

    pub(super) fn next_generation(&self) -> u64 {
        self.next_generation
    }

    pub(super) fn previous_record(&self) -> OptionalSha256V1 {
        match &self.previous_sha256 {
            Some(value) => OptionalSha256V1::Sha256 {
                value: value.clone(),
            },
            None => OptionalSha256V1::Absent,
        }
    }

    pub(super) fn state(&self) -> &AdmissionStateV1 {
        &self.state
    }

    pub(super) fn append(&mut self, value: AdmissionCommitV1) -> Result<String, BoxError> {
        if self.pending_reserved.is_some()
            || value.generation != self.next_generation
            || value.previous_commit != self.previous_record()
        {
            return Err("schedule transaction: admission append generation diverged".into());
        }
        validate_commit_against_state(&value, &self.state)?;
        let bytes = canonical_bytes(&value)?;
        let name = Self::generation_name(self.next_generation);
        let mut file =
            self.directory
                .create_new_file(OsStr::new(&name), 0o600, "admission commit")?;
        // A partial commit is never removed. Its presence makes recovery hold rather than allowing
        // a second admission to reinterpret an ambiguous durable write as absence.
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("schedule transaction: cannot persist commit: {error}"))?;
        self.directory.sync()?;
        let sha256 = local_file::sha256_hex(&bytes);
        self.state = value.admission_state_after.clone();
        if matches!(value.disposition, AdmissionDispositionV1::Reserved { .. }) {
            self.pending_reserved = Some(value);
        }
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or("schedule transaction: admission generation overflow")?;
        self.previous_sha256 = Some(sha256.clone());
        Ok(sha256)
    }

    pub(super) fn append_terminal(
        &mut self,
        value: AdmissionTerminalV1,
    ) -> Result<String, BoxError> {
        let pending = self
            .pending_reserved
            .as_ref()
            .ok_or("schedule transaction: no reserved admission awaits reconciliation")?;
        validate_terminal_against_state(&value, pending, &self.state)?;
        let bytes = canonical_bytes(&value)?;
        let name = Self::terminal_name(value.generation);
        let mut file =
            self.directory
                .create_new_file(OsStr::new(&name), 0o600, "admission terminal")?;
        // A partial terminal is retained so recovery holds instead of reopening the predecessor.
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("schedule transaction: cannot persist terminal: {error}"))?;
        self.directory.sync()?;
        let sha256 = local_file::sha256_hex(&bytes);
        self.state = value.admission_state_after;
        self.pending_reserved = None;
        Ok(sha256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use std::time::Instant;

    use crate::compatibility::{child_terminal_aggregate_fixture, ChildTerminalAggregateFixtureV1};
    use crate::compatibility_process_group::{ProcessIdentityV1, ProcessStartMarkerV1};
    use crate::compatibility_schedule::{
        load_schedule_foundation, EffectCapsV1, EffectClassV1, EvidencePurposeV1,
        FoundationProfileBindingV1, TriggerKindV1,
    };
    use crate::compatibility_schedule_admission::rederive_manual_ledger_context;
    use crate::compatibility_schedule_authority::{
        characterization_record_sha256, derive_manual_admission,
        generate_claimed_support_characterization_source, generate_scheduled_execution_source,
        manual_admission_sha256, seal_characterization_authorization, seal_provider_effect_grant,
        select_characterization_authority, select_standing_grant, AuthorityEnvironmentV1,
        AuthorityStateModelV1, CharacterizationAdmissionRequestV1, FileAuthorityJournal,
        ManualAdmissionBindingsV1, ManualAdmissionOriginV1, ManualNonceSource,
        StandingAdmissionRequestV1,
    };
    use crate::compatibility_schedule_ledger::{
        FileCompatibilityLedger, LedgerBudgetAuthorityV1, LedgerReservationRequestV1,
    };
    use crate::compatibility_schedule_preflight::{
        plan_directory_binding, run_zero_effect_preflight, LocalPreflightProofV1,
        LocalPreflightRefusalV1, PreflightCheckV1, ZeroEffectPreflightChecks,
    };
    use crate::compatibility_schedule_schema::{
        seal_admission_attempt_fingerprint, seal_case_execution_fingerprint,
        AdmissionAttemptFingerprintInputV1, AdmissionAuthorityV1, AdmissionTriggerIdentityV1,
        AggregateBudgetCapsV1, AnchorLifecycleV1, AnchoredProcessGroupRecordV1,
        CandidateBinaryIdentityV1, CaseDeadlineBudgetV1, CaseExecutionFingerprintInputV1,
        CharacterizationAuthorizationV1, CharacterizationOnceAuthorityV1,
        CharacterizationOutcomeV1, CharacterizationRecordV1, CharacterizedGrantProfileV1,
        ChildArtifactJoinV1, ChildArtifactRefV1, DeadlineContainmentV1, DeadlinePhaseBudgetsV1,
        EffectiveIdentityV1, ExactExecutionBindingsV1, ExactExecutionTargetV1, FingerprintV1,
        GitObjectAlgorithmV1, GitObjectIdV1, GrantBudgetPolicyV1, LaunchdBindingV1,
        NamedBudgetCapsV1, OneShotCharacterizationEntryV1, OptionalChildArtifactRefV1,
        OptionalElapsedMsV1, OptionalGitObjectIdV1, OptionalProcessIdentityV1, OptionalRecordRefV1,
        OptionalSafetyHoldReasonV1, OptionalSha256V1, OptionalStableIdV1,
        OptionalSupervisorKillCauseV1, OptionalSupervisorOutcomeV1, OptionalTextV1,
        ProviderEffectGrantV1, SupervisorPhaseV1, TriggerBudgetCapsV1, TriggerSourceV1,
    };
    use crate::compatibility_schedule_state::{AdmissionStateCapability, SchedulerStateRoot};
    use crate::compatibility_schedule_supervisor::{
        FileSupervisorJournal, HardDeadline, PreparedSupervisorV1, SupervisorJournal,
        VerifiedChildArtifact, VerifiedChildTerminalProofV1,
    };

    const COMMIT_AT_MS: i64 = 10;
    const TERMINAL_DEADLINE_MS: i64 = 50_000;
    const AUTHORITY_EXPIRES_AT_MS: i64 = 100_000;

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    fn fingerprint(ch: char) -> FingerprintV1 {
        FingerprintV1 {
            schema_version: 1,
            sha256: digest(ch),
        }
    }

    fn caps() -> EffectCapsV1 {
        EffectCapsV1 {
            timeout_secs: 30,
            max_tokens: 100,
            max_cost_microusd: 1_000,
            attempts: 1,
            retry_cap: 0,
            fallback_cap: 0,
        }
    }

    fn aggregate(attempts: u64) -> AggregateBudgetCapsV1 {
        AggregateBudgetCapsV1 {
            max_attempts: attempts,
            max_tokens: attempts * 100,
            max_cost_microusd: attempts * 1_000,
            max_time_secs: attempts * 30,
        }
    }

    fn budget_policy() -> GrantBudgetPolicyV1 {
        GrantBudgetPolicyV1 {
            per_case: vec![NamedBudgetCapsV1 {
                id: "case-1".into(),
                caps: aggregate(3),
            }],
            per_trigger_pool: Vec::new(),
            per_provider: vec![NamedBudgetCapsV1 {
                id: "provider-1".into(),
                caps: aggregate(3),
            }],
            utc_day: aggregate(3),
            rolling_24h: aggregate(3),
            protected_scheduled: aggregate(1),
            protected_test_merge: aggregate(1),
            manual_unallocated: aggregate(1),
        }
    }

    fn execution_input() -> CaseExecutionFingerprintInputV1 {
        CaseExecutionFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: fingerprint('1'),
            target: ExactExecutionTargetV1::RepositorySnapshot {
                repository: "shoedog/a2acp".into(),
                head_oid: GitObjectIdV1 {
                    algorithm: GitObjectAlgorithmV1::Sha1,
                    hex: "1".repeat(40),
                },
                tree_oid: GitObjectIdV1 {
                    algorithm: GitObjectAlgorithmV1::Sha1,
                    hex: "2".repeat(40),
                },
                range_start_exclusive: OptionalGitObjectIdV1::Absent,
            },
            candidate: CandidateBinaryIdentityV1 {
                sha256: digest('2'),
                length_bytes: 1,
                build_provenance_sha256: digest('3'),
            },
            bindings: ExactExecutionBindingsV1 {
                source_sha256: digest('4'),
                row_sha256: digest('5'),
                run_manifest_sha256: digest('6'),
                generated_config_sha256: digest('7'),
                pin_set_sha256: digest('8'),
                resolution_bundle: OptionalSha256V1::Absent,
                package_integrity_sha256: digest('9'),
                image_digest: OptionalSha256V1::Absent,
                base_image_digest: OptionalSha256V1::Absent,
                environment_sha256: digest('a'),
                prerequisites_sha256: digest('b'),
            },
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
            actual_caps: caps(),
        }
    }

    fn foundation_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility")
    }

    fn foundation_execution(
        binding: &FoundationProfileBindingV1,
    ) -> crate::compatibility_schedule_schema::CaseExecutionFingerprintRecordV1 {
        seal_case_execution_fingerprint(CaseExecutionFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: binding.characterization_profile.clone(),
            target: ExactExecutionTargetV1::RepositorySnapshot {
                repository: "shoedog/a2acp".into(),
                head_oid: GitObjectIdV1 {
                    algorithm: GitObjectAlgorithmV1::Sha1,
                    hex: "a".repeat(40),
                },
                tree_oid: GitObjectIdV1 {
                    algorithm: GitObjectAlgorithmV1::Sha1,
                    hex: "b".repeat(40),
                },
                range_start_exclusive: OptionalGitObjectIdV1::Absent,
            },
            candidate: CandidateBinaryIdentityV1 {
                sha256: digest('c'),
                length_bytes: 1,
                build_provenance_sha256: digest('d'),
            },
            bindings: ExactExecutionBindingsV1 {
                source_sha256: binding.source.source_sha256.clone(),
                row_sha256: binding.source.row_sha256.clone(),
                run_manifest_sha256: digest('e'),
                generated_config_sha256: binding.exact_config_sha256.clone(),
                pin_set_sha256: binding.config_template_sha256.clone(),
                resolution_bundle: OptionalSha256V1::Absent,
                package_integrity_sha256: digest('f'),
                image_digest: OptionalSha256V1::Absent,
                base_image_digest: OptionalSha256V1::Absent,
                environment_sha256: digest('1'),
                prerequisites_sha256: digest('2'),
            },
            requested_identity: binding.requested_identity.clone(),
            expected_effective_identity: binding.expected_effective_identity.clone(),
            actual_caps: binding.maximum_caps.clone(),
        })
        .unwrap()
    }

    fn trigger(
        kind: TriggerKindV1,
        source: TriggerSourceV1,
        suffix: &str,
    ) -> AdmissionTriggerIdentityV1 {
        AdmissionTriggerIdentityV1 {
            source,
            kind,
            request_id: format!("request-{suffix}"),
            window_id: format!("window-{suffix}"),
            attempt_id: format!("attempt-{suffix}"),
            repeat_nonce: OptionalStableIdV1::Absent,
        }
    }

    fn admission_attempt(
        execution: &crate::compatibility_schedule_schema::CaseExecutionFingerprintRecordV1,
        authority: AdmissionAuthorityV1,
        trigger: AdmissionTriggerIdentityV1,
    ) -> crate::compatibility_schedule_schema::AdmissionAttemptFingerprintRecordV1 {
        seal_admission_attempt_fingerprint(AdmissionAttemptFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: execution.input.characterization_profile.clone(),
            case_execution: execution.fingerprint.clone(),
            authority,
            trigger,
        })
        .unwrap()
    }

    fn authority_environment(bundle_sha256: String) -> AuthorityEnvironmentV1 {
        AuthorityEnvironmentV1 {
            operator: "operator".into(),
            environment_owner: "wesleyjinks".into(),
            host_identity_sha256: digest('3'),
            profile_policy_bundle_sha256: bundle_sha256,
            scheduler_binary_sha256: digest('e'),
            price_snapshot_sha256: digest('4'),
            legacy_inventory_sha256: digest('5'),
            now_ms: COMMIT_AT_MS,
            terminal_deadline_ms: TERMINAL_DEADLINE_MS,
        }
    }

    fn authorization_for(
        bundle_sha256: &str,
        binding: &FoundationProfileBindingV1,
        execution: &crate::compatibility_schedule_schema::CaseExecutionFingerprintRecordV1,
    ) -> CharacterizationAuthorizationV1 {
        seal_characterization_authorization(CharacterizationAuthorizationV1 {
            schema_version: 1,
            authorization_id: "authorization-1".into(),
            authorization_sha256: digest('0'),
            operator: "operator".into(),
            environment_owner: "wesleyjinks".into(),
            host_identity_sha256: digest('3'),
            profile_policy_bundle_sha256: bundle_sha256.into(),
            scheduler_binary_sha256: digest('e'),
            price_snapshot_sha256: digest('4'),
            legacy_inventory_sha256: digest('5'),
            issued_at_ms: 2,
            entries: vec![OneShotCharacterizationEntryV1 {
                entry_id: "entry-1".into(),
                generation: 1,
                entry_sha256: digest('0'),
                consumption_nonce: "consumption-1".into(),
                source: binding.source.clone(),
                characterization_profile: binding.characterization_profile.clone(),
                characterization_execution: execution.fingerprint.clone(),
                proposed_effective_identity: binding.expected_effective_identity.clone(),
                provider_family: binding.provider_family.clone(),
                allowed_effects: binding.allowed_effects.clone(),
                caps: execution.input.actual_caps.clone(),
                command: "compatibility characterize".into(),
                not_before_ms: 2,
                expires_at_ms: AUTHORITY_EXPIRES_AT_MS,
                revocation_generation: 1,
                prior_entry: OptionalRecordRefV1::Absent,
                reissue_reason: OptionalTextV1::Absent,
            }],
        })
        .unwrap()
    }

    fn characterization_request(
        binding: &FoundationProfileBindingV1,
        execution: &crate::compatibility_schedule_schema::CaseExecutionFingerprintRecordV1,
    ) -> CharacterizationAdmissionRequestV1 {
        CharacterizationAdmissionRequestV1 {
            entry_id: "entry-1".into(),
            source: binding.source.clone(),
            characterization_profile_sha256: binding.characterization_profile.sha256.clone(),
            characterization_execution_sha256: execution.fingerprint.sha256.clone(),
            proposed_effective_identity: binding.expected_effective_identity.clone(),
            provider_family: binding.provider_family.clone(),
            allowed_effects: binding.allowed_effects.clone(),
            caps: execution.input.actual_caps.clone(),
            command: "compatibility characterize".into(),
            characterization_already_exists: false,
        }
    }

    fn aggregate_for(caps: &EffectCapsV1, attempts: u64) -> AggregateBudgetCapsV1 {
        AggregateBudgetCapsV1 {
            max_attempts: attempts,
            max_tokens: caps.max_tokens * attempts,
            max_cost_microusd: caps.max_cost_microusd * attempts,
            max_time_secs: caps.timeout_secs * attempts,
        }
    }

    fn standing_grant(
        bundle_sha256: &str,
        case_id: &str,
        binding: &FoundationProfileBindingV1,
        characterization: &CharacterizationRecordV1,
    ) -> ProviderEffectGrantV1 {
        let pool = aggregate_for(&binding.maximum_caps, 1);
        seal_provider_effect_grant(ProviderEffectGrantV1 {
            schema_version: 1,
            grant_id: "grant-1".into(),
            generation: 1,
            grant_sha256: digest('0'),
            operator: "operator".into(),
            environment_owner: "wesleyjinks".into(),
            host_identity_sha256: digest('3'),
            profile_policy_bundle_sha256: bundle_sha256.into(),
            scheduler_binary_sha256: digest('e'),
            price_snapshot_sha256: digest('4'),
            price_snapshot_observed_at_ms: 2,
            price_snapshot_valid_until_ms: AUTHORITY_EXPIRES_AT_MS,
            legacy_inventory_sha256: digest('5'),
            triggers: vec![TriggerKindV1::Daily],
            case_ids: vec![case_id.into()],
            provider_families: vec![binding.provider_family.clone()],
            allowed_effects: binding.allowed_effects.clone(),
            per_run_caps: binding.maximum_caps.clone(),
            budgets: GrantBudgetPolicyV1 {
                per_case: vec![NamedBudgetCapsV1 {
                    id: case_id.into(),
                    caps: aggregate_for(&binding.maximum_caps, 3),
                }],
                per_trigger_pool: vec![TriggerBudgetCapsV1 {
                    trigger: TriggerKindV1::Daily,
                    caps: aggregate_for(&binding.maximum_caps, 3),
                }],
                per_provider: vec![NamedBudgetCapsV1 {
                    id: binding.provider_family.clone(),
                    caps: aggregate_for(&binding.maximum_caps, 3),
                }],
                utc_day: aggregate_for(&binding.maximum_caps, 3),
                rolling_24h: aggregate_for(&binding.maximum_caps, 3),
                protected_scheduled: pool.clone(),
                protected_test_merge: pool.clone(),
                manual_unallocated: pool,
            },
            confirmation_allowance: 1,
            launchd: vec![LaunchdBindingV1 {
                label: "com.a2a-bridge.compatibility.daily".into(),
                plist_sha256: digest('6'),
                trigger: TriggerKindV1::Daily,
            }],
            profiles: vec![CharacterizedGrantProfileV1 {
                case_id: case_id.into(),
                provider_family: binding.provider_family.clone(),
                source: binding.source.clone(),
                characterization_profile: binding.characterization_profile.clone(),
                characterization_id: characterization.characterization_id.clone(),
                characterization_sha256: characterization_record_sha256(characterization).unwrap(),
                effective_identity: binding.expected_effective_identity.clone(),
                caps: binding.maximum_caps.clone(),
            }],
            not_before_ms: 2,
            expires_at_ms: AUTHORITY_EXPIRES_AT_MS,
            revocation_generation: 1,
        })
        .unwrap()
    }

    struct FixedNonce;

    impl ManualNonceSource for FixedNonce {
        fn fill(&self, output: &mut [u8]) -> Result<(), BoxError> {
            output.fill(7);
            Ok(())
        }
    }

    struct PassingChecks;

    impl ZeroEffectPreflightChecks for PassingChecks {
        fn revalidate(
            &mut self,
            check: PreflightCheckV1,
        ) -> Result<LocalPreflightProofV1, LocalPreflightRefusalV1> {
            Ok(LocalPreflightProofV1 {
                check,
                evidence_sha256: digest('c'),
                observed_at_ms: COMMIT_AT_MS - 1,
            })
        }
    }

    fn identity(pid: i32, group: i32) -> ProcessIdentityV1 {
        ProcessIdentityV1 {
            pid,
            parent_pid: 1,
            process_group: group,
            session_id: 41,
            start: ProcessStartMarkerV1::MacosEpochMicros {
                seconds: pid as u64,
                microseconds: 0,
            },
        }
    }

    fn deadline_budgets(context: &DerivedLedgerAdmissionContextV1) -> DeadlinePhaseBudgetsV1 {
        DeadlinePhaseBudgetsV1 {
            metadata_fetch_ms: 100,
            checkout_candidate_build_ms: 100,
            preflight_ms: 100,
            resolution_materialization_ms: 100,
            selected_cases: vec![CaseDeadlineBudgetV1 {
                case_id: context.case_id.clone(),
                timeout_ms: 1_000,
            }],
            evidence_publication_ms: 100,
            cold_archive_handoff_ms: 0,
            cleanup_grace_ms: 100,
            fixed_margin_ms: 100,
        }
    }

    fn hard_deadline(context: &DerivedLedgerAdmissionContextV1) -> HardDeadline {
        let trigger = &context.identities.admission_attempt.input.trigger;
        HardDeadline::derive(
            Instant::now(),
            trigger.attempt_id.clone(),
            trigger.window_id.clone(),
            deadline_budgets(context),
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 40_000,
                grant_remaining_ms: 40_000,
                time_budget_remaining_ms: 40_000,
            },
        )
        .unwrap()
    }

    fn prepared_supervisor_record(
        context: &DerivedLedgerAdmissionContextV1,
        deadline_derivation_sha256: String,
    ) -> SupervisorRecordV1 {
        let trigger = context.identities.admission_attempt.input.trigger.kind;
        SupervisorRecordV1 {
            schema_version: 1,
            supervisor_record_id: "supervisor-1".into(),
            generation: 1,
            previous_record: OptionalSha256V1::Absent,
            run_id: context
                .identities
                .admission_attempt
                .input
                .trigger
                .attempt_id
                .clone(),
            window_id: context
                .identities
                .admission_attempt
                .input
                .trigger
                .window_id
                .clone(),
            trigger,
            deadline_derivation_sha256,
            scheduler: identity(42, 42),
            runner: OptionalProcessIdentityV1::Absent,
            groups: vec![AnchoredProcessGroupRecordV1 {
                process_group: 43,
                session_id: 41,
                anchor: identity(43, 43),
                workloads: Vec::new(),
                anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
            }],
            container_run_labels: vec!["a2a-compat-run-1".into()],
            phase: SupervisorPhaseV1::Prepared,
            term_journal_elapsed_ms: OptionalElapsedMsV1::Absent,
            kill_journal_elapsed_ms: OptionalElapsedMsV1::Absent,
            kill_cause: OptionalSupervisorKillCauseV1::Absent,
            later_group_signal_permitted: true,
            outcome: OptionalSupervisorOutcomeV1::Absent,
            safety_hold: OptionalSafetyHoldReasonV1::Absent,
            child_artifact: OptionalChildArtifactRefV1::Absent,
            recorded_at_ms: COMMIT_AT_MS,
        }
    }

    fn prepared_supervisor(context: &DerivedLedgerAdmissionContextV1) -> PreparedSupervisorV1 {
        let deadline = hard_deadline(context);
        let record =
            prepared_supervisor_record(context, deadline.record().derivation.sha256.clone());
        PreparedSupervisorV1::bind(record, deadline).unwrap()
    }

    fn deadline_and_supervisor_for(
        context: &DerivedLedgerAdmissionContextV1,
        run_id: String,
        window_id: String,
        budgets: DeadlinePhaseBudgetsV1,
        containment_ms: u64,
    ) -> (SupervisorRecordV1, HardDeadline) {
        let deadline = HardDeadline::derive(
            Instant::now(),
            run_id.clone(),
            window_id.clone(),
            budgets,
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: containment_ms,
                grant_remaining_ms: containment_ms,
                time_budget_remaining_ms: containment_ms,
            },
        )
        .unwrap();
        let mut supervisor =
            prepared_supervisor_record(context, deadline.record().derivation.sha256.clone());
        supervisor.run_id = run_id;
        supervisor.window_id = window_id;
        (supervisor, deadline)
    }

    fn state_root() -> (tempfile::TempDir, SchedulerStateRoot) {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        (root, state)
    }

    fn action_bindings() -> (
        tempfile::TempDir,
        PlannedDirectoryBindingV1,
        PlannedDirectoryBindingV1,
    ) {
        let action = tempfile::tempdir().unwrap();
        let trusted_root = std::fs::canonicalize(action.path()).unwrap();
        let requested_cwd = trusted_root.join("repo");
        std::fs::create_dir(&requested_cwd).unwrap();
        std::fs::set_permissions(&requested_cwd, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root_binding = plan_directory_binding(&trusted_root).unwrap();
        let cwd_binding = plan_directory_binding(&requested_cwd).unwrap();
        (action, root_binding, cwd_binding)
    }

    fn manual_proposal_for_source<
        C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
    >(
        capability: &C,
        trusted_root: PlannedDirectoryBindingV1,
        requested_cwd: PlannedDirectoryBindingV1,
        input_source_sha256: String,
    ) -> AdmissionCommitProposalV1 {
        let input = execution_input();
        let execution = seal_case_execution_fingerprint(input.clone()).unwrap();
        let manual = derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &FixedNonce,
            ManualAdmissionBindingsV1 {
                operator: "operator".into(),
                environment_owner: "wesleyjinks".into(),
                scheduler_binary_sha256: digest('e'),
                input_source_sha256,
                case_id: "case-1".into(),
                provider_family: "provider-1".into(),
                characterization_profile: input.characterization_profile.clone(),
                case_execution: execution.fingerprint,
                evidence_purpose: EvidencePurposeV1::ManualDiagnostic,
                freshness_bucket: "manual-window".into(),
                caps: caps(),
                allowed_effects: vec![EffectClassV1::ProviderPrompt],
                issued_at_ms: 2,
                expires_at_ms: AUTHORITY_EXPIRES_AT_MS,
            },
        )
        .unwrap();
        let trigger = AdmissionTriggerIdentityV1 {
            source: TriggerSourceV1::ManualCompatibilityCli,
            kind: TriggerKindV1::ManualCompatibility,
            request_id: manual.record.request_nonce.clone(),
            window_id: "manual-window".into(),
            attempt_id: "manual-attempt".into(),
            repeat_nonce: OptionalStableIdV1::Absent,
        };
        let context = rederive_manual_ledger_context(&manual, input, trigger).unwrap();
        let budget_authority = LedgerBudgetAuthorityV1::ManualUnallocated {
            manual_admission_sha256: manual_admission_sha256(&manual.record).unwrap(),
            accounting_grant_sha256: digest('a'),
            budgets: budget_policy(),
        };
        let ledger = FileCompatibilityLedger::open(capability).unwrap();
        let reservation = ledger
            .prepare_reservation(
                &LedgerReservationRequestV1::from_derived_context(&context, &budget_authority),
                COMMIT_AT_MS,
            )
            .unwrap();
        let supervisor = prepared_supervisor(&context);
        let authority_snapshot_sha256 = FileAuthorityJournal::open_existing(capability)
            .unwrap()
            .snapshot_sha256;
        let binding = admission_preflight_binding(
            InternalAdmissionSourceKindV1::GenericManual,
            &manual.record.input_source_sha256,
            &context,
            &AuthorityCommitActionV1::Manual {
                admission: Box::new(manual.clone()),
            },
            &AuthorizedEffectEnvelopeV1 {
                allowed_effects: manual.record.allowed_effects.clone(),
                caps: manual.record.caps.clone(),
            },
            &authority_snapshot_sha256,
            Some(&reservation),
            Some(supervisor.record()),
            Some(supervisor.deadline().record()),
            &trusted_root,
            &requested_cwd,
            TERMINAL_DEADLINE_MS,
            COMMIT_AT_MS,
        )
        .unwrap();
        let initial_preflight = run_zero_effect_preflight(
            PreflightFenceV1::Initial,
            binding.clone(),
            &mut PassingChecks,
        )
        .unwrap();
        let final_preflight =
            run_zero_effect_preflight(PreflightFenceV1::Final, binding, &mut PassingChecks)
                .unwrap();
        AdmissionCommitProposalV1 {
            source_kind: InternalAdmissionSourceKindV1::GenericManual,
            source_sha256: manual.record.input_source_sha256.clone(),
            authority_snapshot_sha256,
            context,
            effect_envelope: AuthorizedEffectEnvelopeV1 {
                allowed_effects: manual.record.allowed_effects.clone(),
                caps: manual.record.caps.clone(),
            },
            authority_action: AuthorityCommitActionV1::Manual {
                admission: Box::new(manual),
            },
            ledger: Some(reservation),
            supervisor: Some(supervisor),
            initial_preflight,
            final_preflight,
            trusted_root,
            requested_cwd,
            terminal_deadline_ms: TERMINAL_DEADLINE_MS,
            recorded_at_ms: COMMIT_AT_MS,
        }
    }

    fn manual_proposal<C: AdmissionStateCapability + AuthorityStateCapability + ?Sized>(
        capability: &C,
        trusted_root: PlannedDirectoryBindingV1,
        requested_cwd: PlannedDirectoryBindingV1,
    ) -> AdmissionCommitProposalV1 {
        manual_proposal_for_source(capability, trusted_root, requested_cwd, digest('f'))
    }

    fn claimed_source_fixture() -> (
        AuthorityStateModelV1,
        ClaimedSupportCharacterizationSourceV1,
        AuthorityEnvironmentV1,
        CharacterizationAdmissionRequestV1,
    ) {
        let root = foundation_root();
        let foundation = load_schedule_foundation(&root).unwrap();
        let (case_id, binding) = foundation.claimed_support_profiles.iter().next().unwrap();
        let execution = foundation_execution(binding);
        let authorization = authorization_for(
            &foundation.profile_policy_bundle_sha256,
            binding,
            &execution,
        );
        let mut state = AuthorityStateModelV1::new();
        state.issue_authorization(authorization).unwrap();
        let environment = authority_environment(foundation.profile_policy_bundle_sha256.clone());
        let request = characterization_request(binding, &execution);
        let authority =
            select_characterization_authority(&state, "authorization-1", &environment, &request)
                .unwrap();
        let admission = admission_attempt(
            &execution,
            authority.clone(),
            trigger(
                TriggerKindV1::ManualCharacterization,
                TriggerSourceV1::ManualCharacterizationCli,
                "claimed",
            ),
        );
        let source = generate_claimed_support_characterization_source(
            &root, case_id, execution, admission, authority,
        )
        .unwrap();
        (state, source, environment, request)
    }

    fn standing_source_fixture() -> (
        AuthorityStateModelV1,
        ScheduledExecutionSourceV1,
        AuthorityEnvironmentV1,
        StandingAdmissionRequestV1,
        FoundationProfileBindingV1,
    ) {
        let root = foundation_root();
        let foundation = load_schedule_foundation(&root).unwrap();
        let (case_id, binding) = foundation.scheduled_profiles.iter().next().unwrap();
        let execution = foundation_execution(binding);
        let characterization_authority =
            AdmissionAuthorityV1::CharacterizationOnce(CharacterizationOnceAuthorityV1 {
                batch_authorization_id: "prior-authorization".into(),
                batch_authorization_sha256: digest('7'),
                entry_id: "prior-entry".into(),
                generation: 1,
                entry_sha256: digest('8'),
                consumption_nonce: "prior-consumption".into(),
            });
        let characterization_admission = admission_attempt(
            &execution,
            characterization_authority.clone(),
            trigger(
                TriggerKindV1::ManualCharacterization,
                TriggerSourceV1::ManualCharacterizationCli,
                "prior-characterization",
            ),
        );
        let characterization = CharacterizationRecordV1 {
            schema_version: 1,
            characterization_id: "characterization-1".into(),
            source: binding.source.clone(),
            profile_policy_bundle_sha256: foundation.profile_policy_bundle_sha256.clone(),
            characterization_profile: binding.characterization_profile.clone(),
            case_execution: execution.fingerprint.clone(),
            admission_attempt: characterization_admission.fingerprint,
            authority: characterization_authority,
            expected_effective_identity: binding.expected_effective_identity.clone(),
            observed_effective_identity: binding.expected_effective_identity.clone(),
            outcome: CharacterizationOutcomeV1::CharacterizedGreen,
            evidence_sha256: digest('9'),
            terminal_at_ms: 5,
        };
        let mut state = AuthorityStateModelV1::new();
        state
            .install_grant(standing_grant(
                &foundation.profile_policy_bundle_sha256,
                case_id,
                binding,
                &characterization,
            ))
            .unwrap();
        let environment = authority_environment(foundation.profile_policy_bundle_sha256.clone());
        let request = StandingAdmissionRequestV1 {
            trigger: TriggerKindV1::Daily,
            case_id: case_id.clone(),
            provider_family: binding.provider_family.clone(),
            source: binding.source.clone(),
            characterization_profile_sha256: binding.characterization_profile.sha256.clone(),
            allowed_effects: binding.allowed_effects.clone(),
            caps: binding.maximum_caps.clone(),
            launchd: Some(LaunchdBindingV1 {
                label: "com.a2a-bridge.compatibility.daily".into(),
                plist_sha256: digest('6'),
                trigger: TriggerKindV1::Daily,
            }),
            characterization,
        };
        let authority = select_standing_grant(&state, "grant-1", &environment, &request).unwrap();
        let admission = admission_attempt(
            &execution,
            authority.clone(),
            trigger(
                TriggerKindV1::Daily,
                TriggerSourceV1::DailyLaunchd,
                "scheduled",
            ),
        );
        let source = generate_scheduled_execution_source(
            &root,
            case_id,
            execution,
            admission,
            authority,
            TriggerKindV1::Daily,
        )
        .unwrap();
        (state, source, environment, request, binding.clone())
    }

    fn fixture_commit() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        SchedulerStateRoot,
        AdmissionCommitV1,
    ) {
        let (state_temp, state) = state_root();
        let (action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = state
            .try_owner_admission("test:transaction-fixture")
            .unwrap()
            .try_authority_state("test:transaction-fixture")
            .unwrap();
        let authority = FileAuthorityJournal::initialize(&locks, 1).unwrap();
        let admission = FileAdmissionJournal::open(&locks).unwrap();
        let proposal = manual_proposal(&locks, trusted_root, requested_cwd);
        let (commit, _deadline) =
            build_admission_commit(&admission.journal, &authority, proposal).unwrap();
        drop(locks);
        (state_temp, action_temp, state, commit)
    }

    fn publish_selected(
        authority_state: AuthorityStateModelV1,
        selected: RederivedSourceAdmissionV1,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        SchedulerStateRoot,
        PublishedAdmissionV1,
    ) {
        let (state_temp, state) = state_root();
        let (action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = state
            .try_owner_admission("test:typed-source")
            .unwrap()
            .try_authority_state("test:typed-source")
            .unwrap();
        let mut authority = FileAuthorityJournal::initialize(&locks, 1).unwrap();
        let (_snapshot, authority_sha256) = authority.journal.append(&authority_state, 2).unwrap();
        let selected = selected.bind_authority_snapshot(&authority_sha256).unwrap();
        let supervisor = prepared_supervisor(&selected.context);
        let proposal = prepare_source_proposal_for_capability(
            &locks,
            selected,
            Some(supervisor),
            trusted_root,
            requested_cwd,
            COMMIT_AT_MS,
            &mut PassingChecks,
            &mut PassingChecks,
        )
        .unwrap();
        let published = commit_prevalidated_proposal_for_capability(&locks, proposal).unwrap();
        drop(locks);
        (state_temp, action_temp, state, published)
    }

    fn publish_fixture_commit(state: &SchedulerStateRoot, commit: &AdmissionCommitV1) {
        let locks = state
            .try_owner_admission("test:publish-fixture")
            .unwrap()
            .try_authority_state("test:publish-fixture")
            .unwrap();
        FileAdmissionJournal::open(&locks)
            .unwrap()
            .journal
            .append(commit.clone())
            .unwrap();
        recover_committed_state(&locks).unwrap();
    }

    fn persist_cancelled_before_running(
        capability: &impl AdmissionStateCapability,
        supervisor_record_id: &str,
    ) {
        let directory = capability.supervisor_directory().canonical_path();
        let (mut journal, prepared, prepared_sha256) =
            FileSupervisorJournal::open_existing(&directory, supervisor_record_id).unwrap();
        let mut reaping = prepared;
        reaping.generation = 2;
        reaping.previous_record = OptionalSha256V1::Sha256 {
            value: prepared_sha256,
        };
        reaping.phase = SupervisorPhaseV1::Reaping;
        reaping.later_group_signal_permitted = false;
        reaping.recorded_at_ms = 11;
        let reaping_sha256 = journal.persist(&reaping).unwrap();

        let mut released = reaping;
        released.generation = 3;
        released.previous_record = OptionalSha256V1::Sha256 {
            value: reaping_sha256,
        };
        for group in &mut released.groups {
            group.anchor_lifecycle = AnchorLifecycleV1::ReleasedReaped;
        }
        released.recorded_at_ms = 12;
        let released_sha256 = journal.persist(&released).unwrap();

        let mut complete = released;
        complete.generation = 4;
        complete.previous_record = OptionalSha256V1::Sha256 {
            value: released_sha256,
        };
        complete.phase = SupervisorPhaseV1::Complete;
        complete.outcome = OptionalSupervisorOutcomeV1::Outcome {
            value: SupervisorTerminalOutcomeV1::CancelledBeforeRunning,
        };
        complete.recorded_at_ms = 13;
        journal.persist(&complete).unwrap();
        FileSupervisorJournal::open_existing(&directory, supervisor_record_id).unwrap();
    }

    fn persist_safety_hold(capability: &impl AdmissionStateCapability, supervisor_record_id: &str) {
        let directory = capability.supervisor_directory().canonical_path();
        let (mut journal, prepared, prepared_sha256) =
            FileSupervisorJournal::open_existing(&directory, supervisor_record_id).unwrap();
        let mut held = prepared;
        held.generation = 2;
        held.previous_record = OptionalSha256V1::Sha256 {
            value: prepared_sha256,
        };
        held.phase = SupervisorPhaseV1::SafetyHold;
        held.later_group_signal_permitted = false;
        held.outcome = OptionalSupervisorOutcomeV1::Outcome {
            value: SupervisorTerminalOutcomeV1::SafetyHold,
        };
        held.safety_hold = OptionalSafetyHoldReasonV1::Reason {
            value: crate::compatibility_schedule_schema::SafetyHoldReasonV1::StartupReconciliationIncomplete,
        };
        held.recorded_at_ms = 11;
        journal.persist(&held).unwrap();
        FileSupervisorJournal::open_existing(&directory, supervisor_record_id).unwrap();
    }

    fn persist_completed(
        capability: &impl AdmissionStateCapability,
        supervisor_record_id: &str,
        child_artifact: ChildArtifactRefV1,
    ) {
        let directory = capability.supervisor_directory().canonical_path();
        let (mut journal, prepared, prepared_sha256) =
            FileSupervisorJournal::open_existing(&directory, supervisor_record_id).unwrap();
        let runner = ProcessIdentityV1 {
            pid: 44,
            parent_pid: 42,
            process_group: 43,
            session_id: 41,
            start: ProcessStartMarkerV1::MacosEpochMicros {
                seconds: 44,
                microseconds: 0,
            },
        };
        let mut running = prepared;
        running.generation = 2;
        running.previous_record = OptionalSha256V1::Sha256 {
            value: prepared_sha256,
        };
        running.runner = OptionalProcessIdentityV1::Process {
            value: runner.clone(),
        };
        running.groups[0].workloads.push(runner);
        running.phase = SupervisorPhaseV1::Running;
        running.recorded_at_ms = 11;
        let running_sha256 = journal.persist(&running).unwrap();

        let mut reaping = running;
        reaping.generation = 3;
        reaping.previous_record = OptionalSha256V1::Sha256 {
            value: running_sha256,
        };
        reaping.phase = SupervisorPhaseV1::Reaping;
        reaping.later_group_signal_permitted = false;
        reaping.child_artifact = OptionalChildArtifactRefV1::Artifact {
            value: child_artifact,
        };
        reaping.recorded_at_ms = 12;
        let reaping_sha256 = journal.persist(&reaping).unwrap();

        let mut released = reaping;
        released.generation = 4;
        released.previous_record = OptionalSha256V1::Sha256 {
            value: reaping_sha256,
        };
        for group in &mut released.groups {
            group.anchor_lifecycle = AnchorLifecycleV1::ReleasedReaped;
        }
        released.recorded_at_ms = 13;
        let released_sha256 = journal.persist(&released).unwrap();

        let mut complete = released;
        complete.generation = 5;
        complete.previous_record = OptionalSha256V1::Sha256 {
            value: released_sha256,
        };
        complete.phase = SupervisorPhaseV1::Complete;
        complete.outcome = OptionalSupervisorOutcomeV1::Outcome {
            value: SupervisorTerminalOutcomeV1::Completed,
        };
        complete.recorded_at_ms = 14;
        journal.persist(&complete).unwrap();
        FileSupervisorJournal::open_existing(&directory, supervisor_record_id).unwrap();
    }

    fn optional_text_value(value: &OptionalTextV1) -> Option<String> {
        match value {
            OptionalTextV1::Absent => None,
            OptionalTextV1::Text { value } => Some(value.clone()),
        }
    }

    fn terminal_aggregate_fixture_for_commit(
        commit: &AdmissionCommitV1,
    ) -> ChildTerminalAggregateFixtureV1 {
        let execution = &commit.context.identities.case_execution.input;
        ChildTerminalAggregateFixtureV1 {
            case_id: commit.context.case_id.clone(),
            candidate_sha256: execution.candidate.sha256.clone(),
            candidate_length_bytes: execution.candidate.length_bytes,
            manifest_sha256: execution.bindings.run_manifest_sha256.clone(),
            requested_model: execution.requested_identity.model.clone(),
            requested_effort: optional_text_value(&execution.requested_identity.effort),
            requested_mode: optional_text_value(&execution.requested_identity.mode),
            observed_model: execution.expected_effective_identity.model.clone(),
            observed_effort: optional_text_value(&execution.expected_effective_identity.effort),
            observed_mode: optional_text_value(&execution.expected_effective_identity.mode),
            tokens: None,
            cost_usd: None,
            duration_ms: 10,
        }
    }

    fn verified_terminal_child(
        supervisor: &SupervisorRecordV1,
        aggregate_bytes: &[u8],
    ) -> Result<(ChildArtifactRefV1, VerifiedChildTerminalProofV1), BoxError> {
        let directory = tempfile::tempdir()?;
        let aggregate_path = directory.path().join("aggregate.json");
        std::fs::write(&aggregate_path, aggregate_bytes)?;
        let aggregate_sha256 = local_file::sha256_hex(aggregate_bytes);
        let join = ChildArtifactJoinV1 {
            schema_version: 1,
            record_id: "artifact-1".into(),
            run_id: supervisor.run_id.clone(),
            window_id: supervisor.window_id.clone(),
            aggregate_sha256: OptionalSha256V1::Sha256 {
                value: aggregate_sha256,
            },
        };
        let mut join_bytes = serde_json::to_vec(&join)?;
        join_bytes.push(b'\n');
        let join_path = directory.path().join("join.json");
        std::fs::write(&join_path, join_bytes)?;
        let verified = VerifiedChildArtifact::load(&join_path, Some(&aggregate_path))?;
        let proof = verified.terminal_proof()?;
        Ok((proof.child_reference().clone(), proof))
    }

    #[test]
    fn preflight_passes_cannot_be_replayed_across_admissions() {
        let (_state_temp, scheduler) = state_root();
        let (_action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = scheduler
            .try_owner_admission("test:preflight-replay")
            .unwrap()
            .try_authority_state("test:preflight-replay")
            .unwrap();
        let authority = FileAuthorityJournal::initialize(&locks, 1).unwrap();
        let admission = FileAdmissionJournal::open(&locks).unwrap();

        let source_a = manual_proposal_for_source(
            &locks,
            trusted_root.clone(),
            requested_cwd.clone(),
            digest('6'),
        );
        let mut source_b =
            manual_proposal_for_source(&locks, trusted_root, requested_cwd, digest('7'));
        source_b.initial_preflight = source_a.initial_preflight;
        source_b.final_preflight = source_a.final_preflight;

        assert!(
            build_admission_commit(&admission.journal, &authority, source_b).is_err(),
            "source A's valid preflight passes must not admit distinct source B"
        );
    }

    #[test]
    fn arbitrary_deadline_digest_cannot_admit() {
        let (_state_temp, scheduler) = state_root();
        let (_action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = scheduler
            .try_owner_admission("test:deadline-binding")
            .unwrap()
            .try_authority_state("test:deadline-binding")
            .unwrap();
        FileAuthorityJournal::initialize(&locks, 1).unwrap();
        let proposal = manual_proposal(&locks, trusted_root, requested_cwd);
        let deadline = hard_deadline(&proposal.context);
        let record = prepared_supervisor_record(&proposal.context, digest('9'));

        assert!(
            PreparedSupervisorV1::bind(record, deadline).is_err(),
            "an arbitrary syntactically valid deadline digest must not admit"
        );
    }

    #[test]
    fn deadline_admission_join_rejects_wrong_run_window_case_cap_and_authority_window() {
        let (_state_temp, scheduler) = state_root();
        let (_action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = scheduler
            .try_owner_admission("test:deadline-join")
            .unwrap()
            .try_authority_state("test:deadline-join")
            .unwrap();
        FileAuthorityJournal::initialize(&locks, 1).unwrap();
        let proposal = manual_proposal(&locks, trusted_root, requested_cwd);
        let context = &proposal.context;
        let ledger = proposal.ledger.as_ref().unwrap();
        let trigger = &context.identities.admission_attempt.input.trigger;
        let assert_refused =
            |label: &str, supervisor: &SupervisorRecordV1, deadline: &HardDeadline| {
                assert!(
                    validate_deadline_record_binding(
                        context,
                        ledger,
                        supervisor,
                        deadline.record(),
                        TERMINAL_DEADLINE_MS,
                        COMMIT_AT_MS,
                    )
                    .is_err(),
                    "{label} must not admit"
                );
            };

        let (wrong_run_supervisor, wrong_run_deadline) = deadline_and_supervisor_for(
            context,
            "another-run".into(),
            trigger.window_id.clone(),
            deadline_budgets(context),
            40_000,
        );
        assert_refused(
            "a deadline for another run",
            &wrong_run_supervisor,
            &wrong_run_deadline,
        );

        let (wrong_window_supervisor, wrong_window_deadline) = deadline_and_supervisor_for(
            context,
            trigger.attempt_id.clone(),
            "another-window".into(),
            deadline_budgets(context),
            40_000,
        );
        assert_refused(
            "a deadline for another schedule window",
            &wrong_window_supervisor,
            &wrong_window_deadline,
        );

        let mut wrong_case_budgets = deadline_budgets(context);
        wrong_case_budgets.selected_cases[0].case_id = "another-case".into();
        let (wrong_case_supervisor, wrong_case_deadline) = deadline_and_supervisor_for(
            context,
            trigger.attempt_id.clone(),
            trigger.window_id.clone(),
            wrong_case_budgets,
            40_000,
        );
        assert_refused(
            "a deadline for another selected case",
            &wrong_case_supervisor,
            &wrong_case_deadline,
        );

        let mut over_cap_budgets = deadline_budgets(context);
        over_cap_budgets.selected_cases[0].timeout_ms = 30_001;
        let (over_cap_supervisor, over_cap_deadline) = deadline_and_supervisor_for(
            context,
            trigger.attempt_id.clone(),
            trigger.window_id.clone(),
            over_cap_budgets,
            40_000,
        );
        assert_refused(
            "a selected-case deadline above the ledger cap",
            &over_cap_supervisor,
            &over_cap_deadline,
        );

        let mut overlong_budgets = deadline_budgets(context);
        overlong_budgets.fixed_margin_ms = 60_000;
        let (overlong_supervisor, overlong_deadline) = deadline_and_supervisor_for(
            context,
            trigger.attempt_id.clone(),
            trigger.window_id.clone(),
            overlong_budgets,
            70_000,
        );
        assert_refused(
            "a deadline outside the authority terminal window",
            &overlong_supervisor,
            &overlong_deadline,
        );
    }

    #[test]
    fn claimed_support_one_shot_reselects_foundation_effects_and_revocation() {
        let (state, source, environment, request) = claimed_source_fixture();
        let mut wrong_effects = request.clone();
        wrong_effects.allowed_effects.clear();
        assert!(
            rederive_claimed_support_characterization_source_against_state(
                &state,
                &foundation_root(),
                &source,
                "freshness-claimed".into(),
                &environment,
                "authorization-1",
                &wrong_effects,
            )
            .is_err()
        );
        let mut revoked = state.clone();
        revoked.rollback_provider_authority().unwrap();
        assert!(
            rederive_claimed_support_characterization_source_against_state(
                &revoked,
                &foundation_root(),
                &source,
                "freshness-claimed".into(),
                &environment,
                "authorization-1",
                &request,
            )
            .is_err()
        );

        let selected = rederive_claimed_support_characterization_source_against_state(
            &state,
            &foundation_root(),
            &source,
            "freshness-claimed".into(),
            &environment,
            "authorization-1",
            &request,
        )
        .unwrap();
        let expected_effects = selected.effect_envelope.allowed_effects.clone();
        let (_state_temp, _action_temp, scheduler, published) = publish_selected(state, selected);
        let PublishedAdmissionV1::Admitted(capability) = published else {
            panic!("one-shot source must reserve");
        };
        assert_eq!(
            capability.effect_envelope().allowed_effects,
            expected_effects
        );
        let supervisor_record_id = capability.supervisor_record_id().to_owned();
        drop(capability);
        let locks = scheduler
            .try_owner_admission("test:one-shot-terminal")
            .unwrap()
            .try_authority_state("test:one-shot-terminal")
            .unwrap();
        persist_cancelled_before_running(&locks, &supervisor_record_id);
        reconcile_pending_admission(
            &locks,
            AdmissionTerminalProofV1::ProvedPreEffect {
                evidence_sha256: digest('6'),
            },
            14,
        )
        .unwrap();
        let authority = FileAuthorityJournal::open_existing(&locks).unwrap();
        assert!(matches!(
            authority
                .snapshot
                .state
                .one_shots
                .get("entry-1")
                .unwrap()
                .phase,
            OneShotLifecyclePhaseV1::Reconciled { .. }
        ));
    }

    #[test]
    fn scheduled_standing_reselects_exact_grant_and_ledger_policy() {
        let (state, source, environment, request, _binding) = standing_source_fixture();
        let mut wrong_effects = request.clone();
        wrong_effects.allowed_effects.clear();
        assert!(rederive_scheduled_standing_source_against_state(
            &state,
            &foundation_root(),
            &source,
            "freshness-scheduled".into(),
            &environment,
            "grant-1",
            &wrong_effects,
        )
        .is_err());
        let mut revoked = state.clone();
        revoked.rollback_provider_authority().unwrap();
        assert!(rederive_scheduled_standing_source_against_state(
            &revoked,
            &foundation_root(),
            &source,
            "freshness-scheduled".into(),
            &environment,
            "grant-1",
            &request,
        )
        .is_err());

        let (_state_temp, scheduler) = state_root();
        let (_action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = scheduler
            .try_owner_admission("test:session-standing")
            .unwrap()
            .try_authority_state("test:session-standing")
            .unwrap();
        let mut journal = FileAuthorityJournal::initialize(&locks, 1).unwrap();
        journal.journal.append(&state, 2).unwrap();
        let session = begin_admission_transaction(&locks).unwrap();
        let selected = session
            .rederive_scheduled_standing_source(
                &foundation_root(),
                &source,
                "freshness-scheduled".into(),
                &environment,
                "grant-1",
                &request,
            )
            .unwrap();
        let expected_effects = selected.effect_envelope.allowed_effects.clone();
        let supervisor = prepared_supervisor(&selected.context);
        let published = session
            .admit(
                selected,
                Some(supervisor),
                trusted_root,
                requested_cwd,
                COMMIT_AT_MS,
                &mut PassingChecks,
                &mut PassingChecks,
            )
            .unwrap();
        let PublishedAdmissionV1::Admitted(capability) = published else {
            panic!("standing source must reserve");
        };
        assert_eq!(
            capability.effect_envelope().allowed_effects,
            expected_effects
        );
    }

    #[test]
    fn completed_standing_work_reuses_without_new_ledger_or_supervisor() {
        let (state, source, environment, request, _binding) = standing_source_fixture();
        let (_state_temp, scheduler) = state_root();
        let (_action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = scheduler
            .try_owner_admission("test:standing-reuse")
            .unwrap()
            .try_authority_state("test:standing-reuse")
            .unwrap();
        let mut authority = FileAuthorityJournal::initialize(&locks, 1).unwrap();
        authority.journal.append(&state, 2).unwrap();

        let session = begin_admission_transaction(&locks).unwrap();
        let selected = session
            .rederive_scheduled_standing_source(
                &foundation_root(),
                &source,
                "freshness-reuse".into(),
                &environment,
                "grant-1",
                &request,
            )
            .unwrap();
        let supervisor = prepared_supervisor(&selected.context);
        let published = session
            .admit(
                selected,
                Some(supervisor),
                trusted_root.clone(),
                requested_cwd.clone(),
                COMMIT_AT_MS,
                &mut PassingChecks,
                &mut PassingChecks,
            )
            .unwrap();
        let PublishedAdmissionV1::Admitted(first) = published else {
            panic!("first standing request must reserve");
        };
        let supervisor_record_id = first.supervisor_record_id().to_owned();
        drop(first);
        drop(session);

        let first_commit = FileAdmissionJournal::open(&locks)
            .unwrap()
            .journal
            .pending_reserved
            .clone()
            .unwrap();
        let first_supervisor = match &first_commit.disposition {
            AdmissionDispositionV1::Reserved { supervisor, .. } => supervisor.as_ref().clone(),
            AdmissionDispositionV1::Reused { .. } => panic!("first standing request must reserve"),
        };
        let aggregate =
            child_terminal_aggregate_fixture(&terminal_aggregate_fixture_for_commit(&first_commit));
        let (child, proof) = verified_terminal_child(&first_supervisor, &aggregate).unwrap();
        persist_completed(&locks, &supervisor_record_id, child);
        let terminal = reconcile_pending_admission(
            &locks,
            AdmissionTerminalProofV1::ValidTerminal {
                child: Box::new(proof),
            },
            15,
        )
        .unwrap();
        let AdmissionTerminalDispositionV1::ValidTerminal { evidence, .. } = terminal.disposition
        else {
            panic!("standing success must produce completed evidence");
        };
        assert!(evidence.reusable);

        let ledger_entries_before = std::fs::read_dir(locks.ledger_directory().canonical_path())
            .unwrap()
            .count();
        let supervisor_entries_before =
            std::fs::read_dir(locks.supervisor_directory().canonical_path())
                .unwrap()
                .count();

        let second_trigger = trigger(
            TriggerKindV1::Daily,
            TriggerSourceV1::DailyLaunchd,
            "scheduled-reuse",
        );
        let second_admission = admission_attempt(
            &source.case_execution,
            source.authority.clone(),
            second_trigger,
        );
        let second_source = generate_scheduled_execution_source(
            &foundation_root(),
            &source.source.row_id,
            source.case_execution.clone(),
            second_admission,
            source.authority.clone(),
            TriggerKindV1::Daily,
        )
        .unwrap();
        let mut second_environment = environment.clone();
        second_environment.now_ms = 16;
        let session = begin_admission_transaction(&locks).unwrap();
        let selected = session
            .rederive_scheduled_standing_source(
                &foundation_root(),
                &second_source,
                "freshness-reuse".into(),
                &second_environment,
                "grant-1",
                &request,
            )
            .unwrap();
        // The prepared supervisor is intentionally supplied. Preview must discard it once the
        // completed evidence proves that this request is a safe-session reuse.
        let supervisor = prepared_supervisor(&selected.context);
        let published = session
            .admit(
                selected,
                Some(supervisor),
                trusted_root,
                requested_cwd,
                16,
                &mut PassingChecks,
                &mut PassingChecks,
            )
            .unwrap();
        let PublishedAdmissionV1::Reused(consumption) = published else {
            panic!("equivalent standing request must reuse");
        };
        assert_eq!(consumption.evidence_sha256, evidence.evidence_sha256);
        assert_eq!(
            std::fs::read_dir(locks.ledger_directory().canonical_path())
                .unwrap()
                .count(),
            ledger_entries_before
        );
        assert_eq!(
            std::fs::read_dir(locks.supervisor_directory().canonical_path())
                .unwrap()
                .count(),
            supervisor_entries_before
        );
        let admission = FileAdmissionJournal::open(&locks).unwrap();
        assert_eq!(admission.commits.len(), 2);
        assert!(matches!(
            admission.commits.last().unwrap().0.disposition,
            AdmissionDispositionV1::Reused { .. }
        ));
    }

    #[test]
    fn r3d_manual_uses_manual_effect_authority_and_only_active_grant_headroom() {
        let (state, _source, environment, _request, binding) = standing_source_fixture();
        let input = foundation_execution(&binding).input;
        let execution = seal_case_execution_fingerprint(input.clone()).unwrap();
        let manual = derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &FixedNonce,
            ManualAdmissionBindingsV1 {
                operator: environment.operator.clone(),
                environment_owner: environment.environment_owner.clone(),
                scheduler_binary_sha256: environment.scheduler_binary_sha256.clone(),
                input_source_sha256: digest('f'),
                case_id: binding.source.row_id.clone(),
                provider_family: binding.provider_family.clone(),
                characterization_profile: binding.characterization_profile.clone(),
                case_execution: execution.fingerprint,
                evidence_purpose: EvidencePurposeV1::ManualDiagnostic,
                freshness_bucket: "freshness-manual".into(),
                caps: input.actual_caps.clone(),
                allowed_effects: binding.allowed_effects.clone(),
                issued_at_ms: 2,
                expires_at_ms: AUTHORITY_EXPIRES_AT_MS,
            },
        )
        .unwrap();
        let manual_trigger = AdmissionTriggerIdentityV1 {
            source: TriggerSourceV1::ManualCompatibilityCli,
            kind: TriggerKindV1::ManualCompatibility,
            request_id: manual.record.request_nonce.clone(),
            window_id: "window-manual-r3d".into(),
            attempt_id: "attempt-manual-r3d".into(),
            repeat_nonce: OptionalStableIdV1::Absent,
        };
        let mut revoked = state.clone();
        revoked.rollback_provider_authority().unwrap();
        assert!(rederive_manual_source_against_state(
            &revoked,
            manual.clone(),
            input.clone(),
            manual_trigger.clone(),
            &environment,
            "grant-1",
        )
        .is_err());
        let selected = rederive_manual_source_against_state(
            &state,
            manual,
            input,
            manual_trigger,
            &environment,
            "grant-1",
        )
        .unwrap();
        assert!(matches!(
            selected.authority_action,
            AuthorityCommitActionV1::Manual { .. }
        ));
        let (_state_temp, _action_temp, _scheduler, published) = publish_selected(state, selected);
        assert!(matches!(published, PublishedAdmissionV1::Admitted(_)));
    }

    #[test]
    fn commit_is_the_single_reducer_derived_linearization_point() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        let owner = state.try_owner_admission("test:append").unwrap();
        let mut open = FileAdmissionJournal::open(&owner).unwrap();
        assert_eq!(open.state, AdmissionStateV1::new());
        let sha256 = open.journal.append(commit.clone()).unwrap();
        assert!(local_file::valid_sha256(&sha256));
        drop(open);
        drop(owner);

        let owner = state.try_owner_admission("test:reopen").unwrap();
        let recovered = FileAdmissionJournal::open(&owner).unwrap();
        assert_eq!(recovered.commits.len(), 1);
        assert_eq!(recovered.commits[0].0, commit);
        assert_eq!(recovered.state, commit.admission_state_after);
    }

    #[test]
    fn commit_rejects_state_not_produced_by_the_equivalent_work_reducer() {
        let (_state_temp, _action_temp, state, mut commit) = fixture_commit();
        commit.admission_state_after = AdmissionStateV1::new();
        let owner = state.try_owner_admission("test:mutation").unwrap();
        let mut open = FileAdmissionJournal::open(&owner).unwrap();
        assert!(open.journal.append(commit).is_err());
        assert_eq!(open.journal.next_generation(), 1);
    }

    #[test]
    fn torn_or_skipped_commit_generation_holds_on_recovery() {
        let (_state_temp, _action_temp, state, _commit) = fixture_commit();
        let owner = state.try_owner_admission("test:torn").unwrap();
        let path = owner
            .admission_directory()
            .canonical_path()
            .join("admission-commit.00000000000000000001.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(b"{").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(FileAdmissionJournal::open(&owner).is_err());
        drop(owner);

        let (_state_temp, _action_temp, state, _commit) = fixture_commit();
        let owner = state.try_owner_admission("test:skipped").unwrap();
        let path = owner
            .admission_directory()
            .canonical_path()
            .join("admission-commit.00000000000000000002.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(b"{}\n").unwrap();
        file.sync_all().unwrap();
        assert!(FileAdmissionJournal::open(&owner).is_err());
    }

    #[test]
    fn noncanonical_committed_bytes_hold_instead_of_being_reinterpreted() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        let owner = state.try_owner_admission("test:canonical").unwrap();
        let mut open = FileAdmissionJournal::open(&owner).unwrap();
        open.journal.append(commit.clone()).unwrap();
        drop(open);
        let path = owner
            .admission_directory()
            .canonical_path()
            .join("admission-commit.00000000000000000001.json");
        let mut bytes = serde_json::to_vec_pretty(&commit).unwrap();
        bytes.push(b'\n');
        std::fs::write(path, bytes).unwrap();
        assert!(FileAdmissionJournal::open(&owner).is_err());
    }

    #[test]
    fn recovery_idempotently_publishes_authority_ledger_and_prepared_supervisor() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        let (reservation_id, supervisor_record_id) = match &commit.disposition {
            AdmissionDispositionV1::Reserved {
                ledger, supervisor, ..
            } => (
                ledger.reservation_id.clone(),
                supervisor.supervisor_record_id.clone(),
            ),
            AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
        };
        let locks = state
            .try_owner_admission("test:crash-after-commit")
            .unwrap()
            .try_authority_state("test:crash-after-commit")
            .unwrap();
        FileAdmissionJournal::open(&locks)
            .unwrap()
            .journal
            .append(commit.clone())
            .unwrap();
        drop(locks);

        for iteration in 1..=2 {
            let locks = state
                .try_owner_admission(&format!("test:recover-{iteration}"))
                .unwrap()
                .try_authority_state(&format!("test:recover-{iteration}"))
                .unwrap();
            assert_eq!(recover_committed_state(&locks).unwrap(), 1);
            let authority = FileAuthorityJournal::open_existing(&locks).unwrap();
            assert_eq!(authority.snapshot.generation, 2);
            assert!(authority.snapshot.state.manual_admissions.contains_key(
                &commit
                    .context
                    .identities
                    .admission_attempt
                    .input
                    .trigger
                    .request_id
            ));
            assert!(locks
                .ledger_directory()
                .canonical_path()
                .join(format!("{reservation_id}.reservation.json"))
                .is_file());
            let (_journal, latest, _sha256) =
                crate::compatibility_schedule_supervisor::FileSupervisorJournal::open_existing(
                    &locks.supervisor_directory().canonical_path(),
                    &supervisor_record_id,
                )
                .unwrap();
            assert_eq!(latest.phase, SupervisorPhaseV1::Prepared);
            assert_eq!(latest.generation, 1);
        }
    }

    #[test]
    fn proved_pre_effect_terminal_releases_once_and_allows_a_regenerated_successor() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        let (ledger_reservation_id, supervisor_record_id, execution_sha256) =
            match &commit.disposition {
                AdmissionDispositionV1::Reserved {
                    ledger, supervisor, ..
                } => (
                    ledger.reservation_id.clone(),
                    supervisor.supervisor_record_id.clone(),
                    ledger.case_execution.sha256.clone(),
                ),
                AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
            };
        publish_fixture_commit(&state, &commit);
        let locks = state
            .try_owner_admission("test:pre-effect-terminal")
            .unwrap()
            .try_authority_state("test:pre-effect-terminal")
            .unwrap();
        persist_cancelled_before_running(&locks, &supervisor_record_id);
        let disposition = AdmissionTerminalProofV1::ProvedPreEffect {
            evidence_sha256: digest('6'),
        };
        let first = reconcile_pending_admission(&locks, disposition.clone(), 14).unwrap();
        let repeated = reconcile_pending_admission(&locks, disposition, 14).unwrap();
        assert_eq!(first, repeated);

        let admission = FileAdmissionJournal::open(&locks).unwrap();
        assert_eq!(admission.terminals.len(), 1);
        assert!(!admission
            .state
            .equivalent_work
            .live_by_execution
            .contains_key(&execution_sha256));
        assert_eq!(admission.state.equivalent_work.completed.len(), 1);
        let ledger = FileCompatibilityLedger::open(&locks).unwrap();
        assert!(ledger.may_admit_regenerated_successor(&ledger_reservation_id));
    }

    #[test]
    fn safety_hold_is_conservatively_charged_and_keeps_equivalent_work_blocked() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        let (ledger_reservation_id, supervisor_record_id, execution_sha256) =
            match &commit.disposition {
                AdmissionDispositionV1::Reserved {
                    ledger, supervisor, ..
                } => (
                    ledger.reservation_id.clone(),
                    supervisor.supervisor_record_id.clone(),
                    ledger.case_execution.sha256.clone(),
                ),
                AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
            };
        publish_fixture_commit(&state, &commit);
        let locks = state
            .try_owner_admission("test:conservative-terminal")
            .unwrap()
            .try_authority_state("test:conservative-terminal")
            .unwrap();
        persist_safety_hold(&locks, &supervisor_record_id);
        reconcile_pending_admission(
            &locks,
            AdmissionTerminalProofV1::Conservative {
                evidence_sha256: digest('7'),
                reason: ConservativeChargeReasonV1::SpawnStateAmbiguous,
                prompt_may_have_been_accepted: true,
            },
            12,
        )
        .unwrap();

        let admission = FileAdmissionJournal::open(&locks).unwrap();
        assert_eq!(admission.terminals.len(), 1);
        assert_eq!(
            admission
                .state
                .equivalent_work
                .live_by_execution
                .get(&execution_sha256),
            Some(&match &commit.disposition {
                AdmissionDispositionV1::Reserved {
                    equivalent_work, ..
                } => equivalent_work.reservation_id.clone(),
                AdmissionDispositionV1::Reused { .. } => unreachable!(),
            })
        );
        let ledger = FileCompatibilityLedger::open(&locks).unwrap();
        assert!(!ledger.may_admit_regenerated_successor(&ledger_reservation_id));
    }

    #[test]
    fn completed_terminal_requires_exact_identity_and_valid_usage_before_it_commits() {
        // A requested/effective identity mismatch is rejected from the immutable child aggregate;
        // there is no caller-supplied nominal identity left to reconcile.
        {
            let (_state_temp, _action_temp, state, commit) = fixture_commit();
            let (supervisor, supervisor_record_id) = match &commit.disposition {
                AdmissionDispositionV1::Reserved { supervisor, .. } => (
                    supervisor.as_ref().clone(),
                    supervisor.supervisor_record_id.clone(),
                ),
                AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
            };
            let mut fixture = terminal_aggregate_fixture_for_commit(&commit);
            fixture.observed_model = "unexpected-model".into();
            let aggregate = child_terminal_aggregate_fixture(&fixture);
            let (child, proof) = verified_terminal_child(&supervisor, &aggregate).unwrap();
            publish_fixture_commit(&state, &commit);
            let locks = state
                .try_owner_admission("test:terminal-identity-drift")
                .unwrap()
                .try_authority_state("test:terminal-identity-drift")
                .unwrap();
            persist_completed(&locks, &supervisor_record_id, child);
            assert!(reconcile_pending_admission(
                &locks,
                AdmissionTerminalProofV1::ValidTerminal {
                    child: Box::new(proof),
                },
                15,
            )
            .is_err());
            assert!(FileAdmissionJournal::open(&locks)
                .unwrap()
                .terminals
                .is_empty());
        }

        // Usage beyond the reservation cap is likewise rejected from aggregate telemetry.
        {
            let (_state_temp, _action_temp, state, commit) = fixture_commit();
            let (ledger, supervisor, supervisor_record_id) = match &commit.disposition {
                AdmissionDispositionV1::Reserved {
                    ledger, supervisor, ..
                } => (
                    ledger.as_ref().clone(),
                    supervisor.as_ref().clone(),
                    supervisor.supervisor_record_id.clone(),
                ),
                AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
            };
            let mut fixture = terminal_aggregate_fixture_for_commit(&commit);
            fixture.tokens = Some(ledger.caps.max_tokens + 1);
            let aggregate = child_terminal_aggregate_fixture(&fixture);
            let (child, proof) = verified_terminal_child(&supervisor, &aggregate).unwrap();
            publish_fixture_commit(&state, &commit);
            let locks = state
                .try_owner_admission("test:terminal-over-cap")
                .unwrap()
                .try_authority_state("test:terminal-over-cap")
                .unwrap();
            persist_completed(&locks, &supervisor_record_id, child);
            assert!(reconcile_pending_admission(
                &locks,
                AdmissionTerminalProofV1::ValidTerminal {
                    child: Box::new(proof),
                },
                15,
            )
            .is_err());
            assert!(FileAdmissionJournal::open(&locks)
                .unwrap()
                .terminals
                .is_empty());
        }

        // A superficially successful aggregate without the one accepted prompt is not proof.
        {
            let (_state_temp, _action_temp, _state, commit) = fixture_commit();
            let supervisor = match &commit.disposition {
                AdmissionDispositionV1::Reserved { supervisor, .. } => supervisor.as_ref().clone(),
                AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
            };
            let fixture = terminal_aggregate_fixture_for_commit(&commit);
            let aggregate: serde_json::Value =
                serde_json::from_slice(&child_terminal_aggregate_fixture(&fixture)).unwrap();
            let mut zero_prompt = aggregate.clone();
            *zero_prompt
                .pointer_mut("/results/0/smoke/turn/prompt_calls")
                .unwrap() = serde_json::json!(0);
            let mut zero_prompt = serde_json::to_vec(&zero_prompt).unwrap();
            zero_prompt.push(b'\n');
            assert!(verified_terminal_child(&supervisor, &zero_prompt).is_err());

            let mut false_token_completeness = aggregate.clone();
            *false_token_completeness
                .pointer_mut("/budget/token_observation_missing_cases")
                .unwrap() = serde_json::json!(0);
            let mut false_token_completeness =
                serde_json::to_vec(&false_token_completeness).unwrap();
            false_token_completeness.push(b'\n');
            assert!(verified_terminal_child(&supervisor, &false_token_completeness).is_err());

            let mut false_cost_total = aggregate;
            *false_cost_total
                .pointer_mut("/budget/observed_cost_usd")
                .unwrap() = serde_json::json!(0.01);
            let mut false_cost_total = serde_json::to_vec(&false_cost_total).unwrap();
            false_cost_total.push(b'\n');
            assert!(verified_terminal_child(&supervisor, &false_cost_total).is_err());
        }

        // Missing token/cost observations charge the immutable reservation caps rather than an
        // untrusted nominal value, while an exact terminal still completes equivalent work.
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        let (ledger, supervisor, supervisor_record_id, execution_sha256) = match &commit.disposition
        {
            AdmissionDispositionV1::Reserved {
                ledger, supervisor, ..
            } => (
                ledger.as_ref().clone(),
                supervisor.as_ref().clone(),
                supervisor.supervisor_record_id.clone(),
                ledger.case_execution.sha256.clone(),
            ),
            AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
        };
        let fixture = terminal_aggregate_fixture_for_commit(&commit);
        assert!(fixture.tokens.is_none() && fixture.cost_usd.is_none());
        let aggregate = child_terminal_aggregate_fixture(&fixture);
        let (child, proof) = verified_terminal_child(&supervisor, &aggregate).unwrap();
        publish_fixture_commit(&state, &commit);
        let locks = state
            .try_owner_admission("test:valid-terminal")
            .unwrap()
            .try_authority_state("test:valid-terminal")
            .unwrap();
        persist_completed(&locks, &supervisor_record_id, child);
        let terminal = reconcile_pending_admission(
            &locks,
            AdmissionTerminalProofV1::ValidTerminal {
                child: Box::new(proof),
            },
            15,
        )
        .unwrap();
        let AdmissionTerminalDispositionV1::ValidTerminal {
            evidence, usage, ..
        } = &terminal.disposition
        else {
            panic!("exact aggregate must produce a valid terminal");
        };
        assert_eq!(usage.attempts, 1);
        assert_eq!(usage.tokens, ledger.caps.max_tokens);
        assert_eq!(usage.cost_microusd, ledger.caps.max_cost_microusd);
        assert_eq!(usage.elapsed_millis, fixture.duration_ms);
        assert_eq!(evidence.evidence_sha256, local_file::sha256_hex(&aggregate));
        assert_eq!(
            evidence.terminal_at_ms,
            1 + i64::try_from(fixture.duration_ms).unwrap()
        );
        assert!(!evidence.reusable);

        let admission = FileAdmissionJournal::open(&locks).unwrap();
        assert_eq!(admission.terminals.len(), 1);
        assert!(!admission
            .state
            .equivalent_work
            .live_by_execution
            .contains_key(&execution_sha256));
        assert_eq!(admission.state.equivalent_work.completed.len(), 1);
        let ledger_state = FileCompatibilityLedger::open(&locks).unwrap();
        assert!(!ledger_state.may_admit_regenerated_successor(&ledger.reservation_id));
    }

    #[test]
    fn terminal_state_mutation_and_successor_before_terminal_fail_closed() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        publish_fixture_commit(&state, &commit);
        let locks = state
            .try_owner_admission("test:terminal-mutation")
            .unwrap()
            .try_authority_state("test:terminal-mutation")
            .unwrap();
        let supervisor_record_id = match &commit.disposition {
            AdmissionDispositionV1::Reserved { supervisor, .. } => {
                supervisor.supervisor_record_id.clone()
            }
            AdmissionDispositionV1::Reused { .. } => unreachable!(),
        };
        let mut journal = FileAdmissionJournal::open(&locks).unwrap();
        assert!(journal.journal.append(commit.clone()).is_err());
        drop(journal);

        persist_cancelled_before_running(&locks, &supervisor_record_id);
        reconcile_pending_admission(
            &locks,
            AdmissionTerminalProofV1::ProvedPreEffect {
                evidence_sha256: digest('8'),
            },
            14,
        )
        .unwrap();
        let path = locks
            .admission_directory()
            .canonical_path()
            .join("admission-terminal.00000000000000000001.json");
        let mut value: AdmissionTerminalV1 =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value.admission_state_after = AdmissionStateV1::new();
        std::fs::write(&path, canonical_bytes(&value).unwrap()).unwrap();
        assert!(FileAdmissionJournal::open(&locks).is_err());
    }

    #[test]
    fn terminal_supervisor_tail_mutation_holds_recovery() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        publish_fixture_commit(&state, &commit);
        let locks = state
            .try_owner_admission("test:terminal-tail-mutation")
            .unwrap()
            .try_authority_state("test:terminal-tail-mutation")
            .unwrap();
        let supervisor_record_id = match &commit.disposition {
            AdmissionDispositionV1::Reserved { supervisor, .. } => {
                supervisor.supervisor_record_id.clone()
            }
            AdmissionDispositionV1::Reused { .. } => unreachable!(),
        };
        persist_cancelled_before_running(&locks, &supervisor_record_id);
        reconcile_pending_admission(
            &locks,
            AdmissionTerminalProofV1::ProvedPreEffect {
                evidence_sha256: digest('8'),
            },
            14,
        )
        .unwrap();
        let path = locks
            .admission_directory()
            .canonical_path()
            .join("admission-terminal.00000000000000000001.json");
        let mut value: AdmissionTerminalV1 =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value.supervisor_journal_sha256 = digest('9');
        std::fs::write(&path, canonical_bytes(&value).unwrap()).unwrap();
        assert!(FileAdmissionJournal::open(&locks).is_ok());
        assert!(recover_committed_state(&locks).is_err());
    }

    #[test]
    fn divergent_authority_publication_holds_without_ledger_or_supervisor() {
        let (_state_temp, _action_temp, state, commit) = fixture_commit();
        let reservation_id = match &commit.disposition {
            AdmissionDispositionV1::Reserved { ledger, .. } => ledger.reservation_id.clone(),
            AdmissionDispositionV1::Reused { .. } => panic!("fixture must reserve"),
        };
        let locks = state
            .try_owner_admission("test:divergent-authority")
            .unwrap()
            .try_authority_state("test:divergent-authority")
            .unwrap();
        FileAdmissionJournal::open(&locks)
            .unwrap()
            .journal
            .append(commit)
            .unwrap();
        let mut authority = FileAuthorityJournal::open_existing(&locks).unwrap();
        let unchanged = authority.snapshot.state.clone();
        authority.journal.append(&unchanged, COMMIT_AT_MS).unwrap();
        assert!(recover_committed_state(&locks).is_err());
        assert!(!locks
            .ledger_directory()
            .canonical_path()
            .join(format!("{reservation_id}.reservation.json"))
            .exists());
        assert_eq!(
            std::fs::read_dir(locks.supervisor_directory().canonical_path())
                .unwrap()
                .count(),
            0
        );
    }

    struct CountingRunner {
        handoffs: usize,
    }

    impl AdmittedRunnerHandoff for CountingRunner {
        type Output = String;

        fn handoff(
            &mut self,
            capability: AdmittedRunCapabilityV1,
        ) -> Result<Self::Output, BoxError> {
            self.handoffs += 1;
            assert_eq!(capability.supervisor_record_id(), "supervisor-1");
            assert_eq!(capability.context().case_id, "case-1");
            assert_eq!(
                capability.effect_envelope().allowed_effects,
                vec![EffectClassV1::ProviderPrompt]
            );
            assert!(capability
                .action_directories()
                .trusted_root
                .current_path_matches());
            Ok(capability.commit_identity_sha256().to_owned())
        }
    }

    #[test]
    fn only_new_fully_published_commit_yields_one_consumable_handoff_capability() {
        let (_state_temp, state) = state_root();
        let (_action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = state
            .try_owner_admission("test:new-admission")
            .unwrap()
            .try_authority_state("test:new-admission")
            .unwrap();
        FileAuthorityJournal::initialize(&locks, 1).unwrap();
        let proposal = manual_proposal(&locks, trusted_root, requested_cwd);
        let expected_deadline_sha256 = proposal
            .supervisor
            .as_ref()
            .unwrap()
            .deadline()
            .record()
            .derivation
            .sha256
            .clone();
        let PublishedAdmissionV1::Admitted(capability) =
            commit_prevalidated_proposal_for_capability(&locks, proposal).unwrap()
        else {
            panic!("manual fixture must create a new admission");
        };
        assert_eq!(
            capability.hard_deadline().record().derivation.sha256,
            expected_deadline_sha256
        );
        assert!(!capability.hard_deadline().remaining().is_zero());
        let mut runner = CountingRunner { handoffs: 0 };
        let commit_identity = handoff_admitted(*capability, &mut runner).unwrap();
        assert!(local_file::valid_sha256(&commit_identity));
        assert_eq!(runner.handoffs, 1);
        drop(locks);

        let locks = state
            .try_owner_admission("test:recovery-no-handoff")
            .unwrap()
            .try_authority_state("test:recovery-no-handoff")
            .unwrap();
        assert_eq!(recover_committed_state(&locks).unwrap(), 1);
        assert_eq!(runner.handoffs, 1);
    }

    #[test]
    fn action_directory_drift_refuses_before_the_admission_commit() {
        let (_state_temp, state) = state_root();
        let (_action_temp, trusted_root, requested_cwd) = action_bindings();
        let locks = state
            .try_owner_admission("test:directory-drift")
            .unwrap()
            .try_authority_state("test:directory-drift")
            .unwrap();
        FileAuthorityJournal::initialize(&locks, 1).unwrap();
        let proposal = manual_proposal(&locks, trusted_root, requested_cwd.clone());
        let cwd = std::path::PathBuf::from(&requested_cwd.canonical_path);
        let moved = cwd.with_file_name("repo-old");
        std::fs::rename(&cwd, moved).unwrap();
        std::fs::create_dir(&cwd).unwrap();
        std::fs::set_permissions(&cwd, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(commit_prevalidated_proposal_for_capability(&locks, proposal).is_err());
        assert!(FileAdmissionJournal::open(&locks)
            .unwrap()
            .commits
            .is_empty());
        assert_eq!(
            FileAuthorityJournal::open_existing(&locks)
                .unwrap()
                .snapshot
                .generation,
            1
        );
    }
}
