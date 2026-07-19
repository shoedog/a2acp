//! Crash-consistent R3d2 admission linearization and recovery.
//!
//! A complete owner-private admission commit is the only linearization point. Authority and
//! ledger journals are previewed before that commit and published idempotently after it. No type
//! in this module can call a provider; only the opaque capability created after publication may be
//! transferred to an injected runner handoff.

#![allow(dead_code)] // The default-off CLI wiring lands at the end of R3d2e.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule::{
    load_schedule_foundation, EffectCapsV1, EffectClassV1, TriggerKindV1,
};
use crate::compatibility_schedule_admission::{
    rederive_claimed_support_ledger_context_from_foundation, rederive_manual_ledger_context,
    rederive_scheduled_ledger_context_from_foundation, CharacterizationStateV1, ControlStateV1,
    DerivedLedgerAdmissionContextV1, EquivalentWorkDecisionV1, EquivalentWorkStateV1,
};
use crate::compatibility_schedule_authority::{
    authority_state_snapshot_sha256, manual_admission_sha256, select_characterization_authority,
    select_manual_accounting_grant, select_standing_grant, AuthorityEnvironmentV1,
    AuthorityJournalOpen, AuthorityStateModelV1, AuthorityStateSnapshotV1,
    CharacterizationAdmissionRequestV1, FileAuthorityJournal, OneShotLifecyclePhaseV1,
    SealedManualAdmissionV1, StandingAdmissionRequestV1,
};
use crate::compatibility_schedule_ledger::{
    prepared_reservation_sha256, validate_prepared_reservation_context, FileCompatibilityLedger,
    LedgerBudgetAuthorityV1, LedgerReservationRequestV1,
};
use crate::compatibility_schedule_preflight::{
    pin_action_directories, preflight_pass_sha256, validate_planned_directory_binding,
    PinnedActionDirectoriesV1, PlannedDirectoryBindingV1, PreflightFenceV1, PreflightPassV1,
};
use crate::compatibility_schedule_schema::{
    validate_supervisor_record, AdmissionAuthorityV1, AdmissionTriggerIdentityV1,
    CaseExecutionFingerprintInputV1, ClaimedSupportCharacterizationSourceV1, ConsumptionRecordV1,
    EquivalentWorkReservationV1, LedgerReservationV1, OptionalSha256V1, ScheduledExecutionSourceV1,
    SupervisorRecordV1,
};
use crate::compatibility_schedule_state::{AdmissionStateCapability, AuthorityStateCapability};
use crate::compatibility_schedule_supervisor::ensure_prepared_supervisor;
use crate::{local_file, BoxError};

const MAX_COMMIT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COMMIT_GENERATIONS: usize = 100_000;
const COMMIT_PREFIX: &str = "admission-commit.";

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
    OneShot { entry_id: String },
    Manual { admission: SealedManualAdmissionV1 },
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
        ledger: LedgerReservationV1,
        ledger_sha256: String,
        supervisor: SupervisorRecordV1,
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
    pub(super) recorded_at_ms: i64,
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
            recorded_at_ms: value.recorded_at_ms,
        },
    )
}

fn validate_commit_against_state(
    value: &AdmissionCommitV1,
    before: &AdmissionStateV1,
) -> Result<(), BoxError> {
    if value.schema_version != 1 || value.generation == 0 || value.recorded_at_ms <= 0 {
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
    if value.initial_preflight.fence != PreflightFenceV1::Initial
        || value.final_preflight.fence != PreflightFenceV1::Final
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
                supervisor,
                supervisor_sha256,
            },
            EquivalentWorkDecisionV1::Reserved(expected),
        ) if equivalent_work == &expected => {
            validate_prepared_reservation_context(ledger, &value.context)?;
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
    pub(super) context: DerivedLedgerAdmissionContextV1,
    pub(super) authority_action: AuthorityCommitActionV1,
    pub(super) effect_envelope: AuthorizedEffectEnvelopeV1,
    pub(super) ledger: Option<LedgerReservationV1>,
    pub(super) supervisor: Option<SupervisorRecordV1>,
    pub(super) initial_preflight: PreflightPassV1,
    pub(super) final_preflight: PreflightPassV1,
    pub(super) trusted_root: PlannedDirectoryBindingV1,
    pub(super) requested_cwd: PlannedDirectoryBindingV1,
    pub(super) recorded_at_ms: i64,
}

pub(super) struct RederivedSourceAdmissionV1 {
    source_kind: InternalAdmissionSourceKindV1,
    source_sha256: String,
    context: DerivedLedgerAdmissionContextV1,
    authority_action: AuthorityCommitActionV1,
    effect_envelope: AuthorizedEffectEnvelopeV1,
    budget_authority: LedgerBudgetAuthorityV1,
    selected_at_ms: i64,
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
            admission: manual.clone(),
        },
        effect_envelope,
        budget_authority: LedgerBudgetAuthorityV1::ManualUnallocated {
            manual_admission_sha256: manual_admission_sha256(&manual.record)?,
            accounting_grant_sha256: grant.grant_sha256.clone(),
            budgets: grant.budgets.clone(),
        },
        selected_at_ms: environment.now_ms,
        authority_snapshot_sha256: String::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_source_proposal_for_capability<C>(
    capability: &C,
    selected: RederivedSourceAdmissionV1,
    supervisor: Option<SupervisorRecordV1>,
    initial_preflight: PreflightPassV1,
    final_preflight: PreflightPassV1,
    trusted_root: PlannedDirectoryBindingV1,
    requested_cwd: PlannedDirectoryBindingV1,
    recorded_at_ms: i64,
) -> Result<AdmissionCommitProposalV1, BoxError>
where
    C: AdmissionStateCapability + AuthorityStateCapability + ?Sized,
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
    let ledger = FileCompatibilityLedger::open(capability)?;
    let reservation = ledger.prepare_reservation(
        &LedgerReservationRequestV1::from_derived_context(
            &selected.context,
            &selected.budget_authority,
        ),
        recorded_at_ms,
    )?;
    Ok(AdmissionCommitProposalV1 {
        source_kind: selected.source_kind,
        source_sha256: selected.source_sha256,
        context: selected.context,
        authority_action: selected.authority_action,
        effect_envelope: selected.effect_envelope,
        ledger: Some(reservation),
        supervisor,
        initial_preflight,
        final_preflight,
        trusted_root,
        requested_cwd,
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
    pub(super) fn prepare_proposal(
        &self,
        selected: RederivedSourceAdmissionV1,
        supervisor: Option<SupervisorRecordV1>,
        initial_preflight: PreflightPassV1,
        final_preflight: PreflightPassV1,
        trusted_root: PlannedDirectoryBindingV1,
        requested_cwd: PlannedDirectoryBindingV1,
        recorded_at_ms: i64,
    ) -> Result<AdmissionCommitProposalV1, BoxError> {
        prepare_source_proposal_for_capability(
            self.capability,
            selected,
            supervisor,
            initial_preflight,
            final_preflight,
            trusted_root,
            requested_cwd,
            recorded_at_ms,
        )
    }

    pub(super) fn commit(
        &self,
        proposal: AdmissionCommitProposalV1,
    ) -> Result<PublishedAdmissionV1, BoxError> {
        commit_prevalidated_proposal_for_capability(self.capability, proposal)
    }
}

pub(super) fn build_admission_commit(
    admission: &FileAdmissionJournal<'_>,
    authority: &AuthorityJournalOpen<'_>,
    proposal: AdmissionCommitProposalV1,
) -> Result<AdmissionCommitV1, BoxError> {
    proposal.context.identities.validate()?;
    if proposal.recorded_at_ms <= 0 || !local_file::valid_sha256(&proposal.source_sha256) {
        return Err("schedule transaction: proposal time/source binding is invalid".into());
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
    let disposition = match (decision, proposal.ledger, proposal.supervisor) {
        (EquivalentWorkDecisionV1::Reserved(equivalent_work), Some(ledger), Some(supervisor)) => {
            validate_prepared_reservation_context(&ledger, &proposal.context)?;
            validate_supervisor_record(&supervisor)?;
            AdmissionDispositionV1::Reserved {
                equivalent_work,
                ledger_sha256: prepared_reservation_sha256(&ledger)?,
                supervisor_sha256: supervisor_record_sha256(&supervisor)?,
                ledger,
                supervisor,
            }
        }
        (EquivalentWorkDecisionV1::Reused(consumption), None, None) => {
            AdmissionDispositionV1::Reused { consumption }
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
                admission.clone(),
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
    Ok(commit)
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
                opened_ledger.commit_prepared_reservation(ledger.clone())?;
            if &published != ledger {
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
    Ok(admission.commits.len())
}

pub(super) struct AdmittedRunCapabilityV1 {
    commit_identity_sha256: String,
    context: DerivedLedgerAdmissionContextV1,
    effect_envelope: AuthorizedEffectEnvelopeV1,
    supervisor_record_id: String,
    action_directories: PinnedActionDirectoriesV1,
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
}

pub(super) enum PublishedAdmissionV1 {
    Admitted(AdmittedRunCapabilityV1),
    Reused(ConsumptionRecordV1),
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
    let commit = build_admission_commit(&admission.journal, &authority, proposal)?;
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
            if &latest_supervisor == supervisor =>
        {
            Ok(PublishedAdmissionV1::Admitted(AdmittedRunCapabilityV1 {
                commit_identity_sha256: commit.commit_identity_sha256,
                context: commit.context,
                effect_envelope: commit.effect_envelope,
                supervisor_record_id: supervisor.supervisor_record_id.clone(),
                action_directories,
            }))
        }
        (AdmissionDispositionV1::Reused { consumption }, None) => {
            Ok(PublishedAdmissionV1::Reused(consumption.clone()))
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
}

#[derive(Clone, Debug)]
pub(super) struct FileAdmissionJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    next_generation: u64,
    previous_sha256: Option<String>,
    state: AdmissionStateV1,
}

impl<'lock> FileAdmissionJournal<'lock> {
    fn generation_name(generation: u64) -> String {
        format!("{COMMIT_PREFIX}{generation:020}.json")
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

    pub(super) fn open<C: AdmissionStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<AdmissionJournalOpen<'lock>, BoxError> {
        let directory = capability.admission_directory();
        let mut state = AdmissionStateV1::new();
        let mut previous_sha256: Option<String> = None;
        let mut commits = Vec::new();
        for (index, (generation, name)) in Self::entries(directory)?.into_iter().enumerate() {
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
            commits.push((commit, sha256));
        }
        state.validate()?;
        Ok(AdmissionJournalOpen {
            journal: Self {
                directory,
                next_generation: u64::try_from(commits.len())?.saturating_add(1),
                previous_sha256,
                state: state.clone(),
            },
            state,
            commits,
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
        if value.generation != self.next_generation
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
        self.state = value.admission_state_after;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or("schedule transaction: admission generation overflow")?;
        self.previous_sha256 = Some(sha256.clone());
        Ok(sha256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

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
        CandidateBinaryIdentityV1, CaseExecutionFingerprintInputV1,
        CharacterizationAuthorizationV1, CharacterizationOnceAuthorityV1,
        CharacterizationOutcomeV1, CharacterizationRecordV1, CharacterizedGrantProfileV1,
        EffectiveIdentityV1, ExactExecutionBindingsV1, ExactExecutionTargetV1, FingerprintV1,
        GitObjectAlgorithmV1, GitObjectIdV1, GrantBudgetPolicyV1, LaunchdBindingV1,
        NamedBudgetCapsV1, OneShotCharacterizationEntryV1, OptionalChildArtifactRefV1,
        OptionalElapsedMsV1, OptionalGitObjectIdV1, OptionalProcessIdentityV1, OptionalRecordRefV1,
        OptionalSafetyHoldReasonV1, OptionalSha256V1, OptionalStableIdV1,
        OptionalSupervisorKillCauseV1, OptionalSupervisorOutcomeV1, OptionalTextV1,
        ProviderEffectGrantV1, SupervisorPhaseV1, TriggerBudgetCapsV1, TriggerSourceV1,
    };
    use crate::compatibility_schedule_state::{AdmissionStateCapability, SchedulerStateRoot};

    const COMMIT_AT_MS: i64 = 10;

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
            terminal_deadline_ms: 50,
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
                expires_at_ms: 100,
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
            price_snapshot_valid_until_ms: 100,
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
            expires_at_ms: 100,
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

    fn prepared_supervisor(context: &DerivedLedgerAdmissionContextV1) -> SupervisorRecordV1 {
        let trigger = context.identities.admission_attempt.input.trigger.kind;
        SupervisorRecordV1 {
            schema_version: 1,
            supervisor_record_id: "supervisor-1".into(),
            generation: 1,
            previous_record: OptionalSha256V1::Absent,
            run_id: "run-1".into(),
            window_id: context
                .identities
                .admission_attempt
                .input
                .trigger
                .window_id
                .clone(),
            trigger,
            deadline_derivation_sha256: digest('d'),
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

    fn manual_proposal<C: AdmissionStateCapability + ?Sized>(
        capability: &C,
        trusted_root: PlannedDirectoryBindingV1,
        requested_cwd: PlannedDirectoryBindingV1,
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
                input_source_sha256: digest('f'),
                case_id: "case-1".into(),
                provider_family: "provider-1".into(),
                characterization_profile: input.characterization_profile.clone(),
                case_execution: execution.fingerprint,
                evidence_purpose: EvidencePurposeV1::ManualDiagnostic,
                freshness_bucket: "manual-window".into(),
                caps: caps(),
                allowed_effects: vec![EffectClassV1::ProviderPrompt],
                issued_at_ms: 2,
                expires_at_ms: 100,
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
        let initial_preflight =
            run_zero_effect_preflight(PreflightFenceV1::Initial, &mut PassingChecks).unwrap();
        let final_preflight =
            run_zero_effect_preflight(PreflightFenceV1::Final, &mut PassingChecks).unwrap();
        AdmissionCommitProposalV1 {
            source_kind: InternalAdmissionSourceKindV1::GenericManual,
            source_sha256: manual.record.input_source_sha256.clone(),
            context,
            effect_envelope: AuthorizedEffectEnvelopeV1 {
                allowed_effects: manual.record.allowed_effects.clone(),
                caps: manual.record.caps.clone(),
            },
            authority_action: AuthorityCommitActionV1::Manual { admission: manual },
            ledger: Some(reservation),
            supervisor: Some(supervisor),
            initial_preflight,
            final_preflight,
            trusted_root,
            requested_cwd,
            recorded_at_ms: COMMIT_AT_MS,
        }
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
        let commit = build_admission_commit(&admission.journal, &authority, proposal).unwrap();
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
        let initial =
            run_zero_effect_preflight(PreflightFenceV1::Initial, &mut PassingChecks).unwrap();
        let final_ =
            run_zero_effect_preflight(PreflightFenceV1::Final, &mut PassingChecks).unwrap();
        let proposal = prepare_source_proposal_for_capability(
            &locks,
            selected,
            Some(supervisor),
            initial,
            final_,
            trusted_root,
            requested_cwd,
            COMMIT_AT_MS,
        )
        .unwrap();
        let published = commit_prevalidated_proposal_for_capability(&locks, proposal).unwrap();
        drop(locks);
        (state_temp, action_temp, state, published)
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
        let (_state_temp, _action_temp, _scheduler, published) = publish_selected(state, selected);
        let PublishedAdmissionV1::Admitted(capability) = published else {
            panic!("one-shot source must reserve");
        };
        assert_eq!(
            capability.effect_envelope().allowed_effects,
            expected_effects
        );
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
        let proposal = session
            .prepare_proposal(
                selected,
                Some(supervisor),
                run_zero_effect_preflight(PreflightFenceV1::Initial, &mut PassingChecks).unwrap(),
                run_zero_effect_preflight(PreflightFenceV1::Final, &mut PassingChecks).unwrap(),
                trusted_root,
                requested_cwd,
                COMMIT_AT_MS,
            )
            .unwrap();
        let published = session.commit(proposal).unwrap();
        let PublishedAdmissionV1::Admitted(capability) = published else {
            panic!("standing source must reserve");
        };
        assert_eq!(
            capability.effect_envelope().allowed_effects,
            expected_effects
        );
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
                expires_at_ms: 100,
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
        let PublishedAdmissionV1::Admitted(capability) =
            commit_prevalidated_proposal_for_capability(&locks, proposal).unwrap()
        else {
            panic!("manual fixture must create a new admission");
        };
        let mut runner = CountingRunner { handoffs: 0 };
        let commit_identity = handoff_admitted(capability, &mut runner).unwrap();
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
