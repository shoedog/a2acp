//! Sealed private authority and one-shot lifecycle reducers for R3d2.
//!
//! This module is still effect-free with respect to providers. It validates owner-issued records,
//! derives their canonical hashes, and reduces append-only durable authority state while the caller
//! holds the authority-state capability. No schedule entrypoint reaches it until the shared R3d2e
//! transaction is complete.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule::{
    load_schedule_foundation, EffectCapsV1, EffectClassV1, EvidencePurposeV1,
    FoundationProfileBindingV1, TriggerKindV1,
};
use crate::compatibility_schedule_schema::{
    AdmissionAttemptFingerprintRecordV1, AdmissionAuthorityV1, CaseExecutionFingerprintRecordV1,
    CharacterizationAuthorizationV1, CharacterizationOnceAuthorityV1, CharacterizationOutcomeV1,
    CharacterizationRecordV1, ClaimedSupportCharacterizationSourceV1, EvidenceClassV1,
    LaunchdBindingV1, ManualAcknowledgementAuthorityV1, ManualAdmissionSourceV1, ManualAdmissionV1,
    ManualCommandV1, OneShotCharacterizationEntryV1, OptionalRecordRefV1, OptionalSha256V1,
    OptionalTextV1, ProfileSourceRefV1, ProviderEffectGrantV1, ScheduledExecutionSourceV1,
    StandingGrantAuthorityV1, StorageConsentV1, ValidateRecord,
};
use crate::compatibility_schedule_state::AuthorityStateCapability;
use crate::{local_file, BoxError};

const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn authority_hash<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| format!("schedule authority: cannot canonicalize {label}: {error}"))?;
    let mut bytes = format!("a2a-bridge:r3d2:{label}:v1\0").into_bytes();
    bytes.extend_from_slice(&canonical);
    Ok(local_file::sha256_hex(&bytes))
}

fn canonicalize_entry(value: &mut OneShotCharacterizationEntryV1) {
    value.allowed_effects.sort();
}

fn canonicalize_authorization(value: &mut CharacterizationAuthorizationV1) {
    for entry in &mut value.entries {
        canonicalize_entry(entry);
    }
    value
        .entries
        .sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
}

fn canonicalize_grant(value: &mut ProviderEffectGrantV1) {
    value.triggers.sort();
    value.case_ids.sort();
    value.provider_families.sort();
    value.allowed_effects.sort();
    value
        .budgets
        .per_case
        .sort_by(|left, right| left.id.cmp(&right.id));
    value
        .budgets
        .per_trigger_pool
        .sort_by_key(|item| item.trigger);
    value
        .budgets
        .per_provider
        .sort_by(|left, right| left.id.cmp(&right.id));
    value
        .launchd
        .sort_by(|left, right| left.label.cmp(&right.label));
    value
        .profiles
        .sort_by(|left, right| left.case_id.cmp(&right.case_id));
}

fn canonicalize_consent(value: &mut StorageConsentV1) {
    value.evidence_classes.sort();
}

pub(super) fn one_shot_entry_sha256(
    value: &OneShotCharacterizationEntryV1,
) -> Result<String, BoxError> {
    let mut value = value.clone();
    canonicalize_entry(&mut value);
    value.entry_sha256 = ZERO_SHA256.into();
    authority_hash("one-shot-characterization-entry", &value)
}

pub(super) fn seal_characterization_authorization(
    mut value: CharacterizationAuthorizationV1,
) -> Result<CharacterizationAuthorizationV1, BoxError> {
    canonicalize_authorization(&mut value);
    for entry in &mut value.entries {
        entry.entry_sha256 = one_shot_entry_sha256(entry)?;
    }
    value.authorization_sha256 = ZERO_SHA256.into();
    value.authorization_sha256 = authority_hash("characterization-authorization", &value)?;
    value.validate()?;
    Ok(value)
}

pub(super) fn validate_sealed_characterization_authorization(
    value: &CharacterizationAuthorizationV1,
) -> Result<(), BoxError> {
    value.validate()?;
    let sealed = seal_characterization_authorization(value.clone())?;
    if &sealed != value {
        return Err(
            "schedule authority: characterization authorization is noncanonical or has a stale hash"
                .into(),
        );
    }
    Ok(())
}

pub(super) fn seal_provider_effect_grant(
    mut value: ProviderEffectGrantV1,
) -> Result<ProviderEffectGrantV1, BoxError> {
    canonicalize_grant(&mut value);
    value.grant_sha256 = ZERO_SHA256.into();
    value.grant_sha256 = authority_hash("provider-effect-grant", &value)?;
    value.validate()?;
    Ok(value)
}

pub(super) fn validate_sealed_provider_effect_grant(
    value: &ProviderEffectGrantV1,
) -> Result<(), BoxError> {
    value.validate()?;
    let sealed = seal_provider_effect_grant(value.clone())?;
    if &sealed != value {
        return Err(
            "schedule authority: provider-effect grant is noncanonical or has a stale hash".into(),
        );
    }
    Ok(())
}

pub(super) fn seal_storage_consent(
    mut value: StorageConsentV1,
) -> Result<StorageConsentV1, BoxError> {
    canonicalize_consent(&mut value);
    value.consent_sha256 = ZERO_SHA256.into();
    value.consent_sha256 = authority_hash("storage-consent", &value)?;
    value.validate()?;
    Ok(value)
}

pub(super) fn validate_sealed_storage_consent(value: &StorageConsentV1) -> Result<(), BoxError> {
    value.validate()?;
    let sealed = seal_storage_consent(value.clone())?;
    if &sealed != value {
        return Err(
            "schedule authority: storage consent is noncanonical or has a stale hash".into(),
        );
    }
    Ok(())
}

pub(super) fn characterization_record_sha256(
    value: &CharacterizationRecordV1,
) -> Result<String, BoxError> {
    value.validate()?;
    authority_hash("characterization-record", value)
}

fn scheduled_execution_source_sha256(
    value: &ScheduledExecutionSourceV1,
) -> Result<String, BoxError> {
    let mut value = value.clone();
    value.source_sha256 = ZERO_SHA256.into();
    authority_hash("scheduled-execution-source", &value)
}

fn claimed_support_source_sha256(
    value: &ClaimedSupportCharacterizationSourceV1,
) -> Result<String, BoxError> {
    let mut value = value.clone();
    value.source_sha256 = ZERO_SHA256.into();
    authority_hash("claimed-support-characterization-source", &value)
}

struct FoundationBindingObservationV1<'a> {
    source: &'a ProfileSourceRefV1,
    characterization_profile: &'a crate::compatibility_schedule_schema::FingerprintV1,
    requested_identity: &'a crate::compatibility_schedule_schema::EffectiveIdentityV1,
    expected_effective_identity: &'a crate::compatibility_schedule_schema::EffectiveIdentityV1,
    caps: &'a EffectCapsV1,
    config_template_sha256: &'a str,
    generated_config_sha256: &'a str,
}

fn validate_foundation_binding(
    binding: &FoundationProfileBindingV1,
    observed: FoundationBindingObservationV1<'_>,
) -> Result<(), BoxError> {
    observed.caps.validate("source actual caps")?;
    observed
        .caps
        .within(&binding.maximum_caps, "source actual caps")?;
    if observed.source != &binding.source
        || observed.characterization_profile != &binding.characterization_profile
        || observed.requested_identity != &binding.requested_identity
        || observed.expected_effective_identity != &binding.expected_effective_identity
        || observed.config_template_sha256 != binding.config_template_sha256
        || observed.generated_config_sha256 != binding.exact_config_sha256
    {
        return Err(
            "schedule authority: source does not match the rederived checked-in foundation".into(),
        );
    }
    Ok(())
}

pub(super) fn seal_scheduled_execution_source(
    mut value: ScheduledExecutionSourceV1,
) -> Result<ScheduledExecutionSourceV1, BoxError> {
    value.source_sha256 = ZERO_SHA256.into();
    value.validate()?;
    value.source_sha256 = scheduled_execution_source_sha256(&value)?;
    value.validate()?;
    Ok(value)
}

pub(super) fn validate_scheduled_execution_source(
    foundation_root: &Path,
    value: &ScheduledExecutionSourceV1,
) -> Result<(), BoxError> {
    value.validate()?;
    if value.source_sha256 != scheduled_execution_source_sha256(value)? {
        return Err(
            "schedule authority: scheduled source is noncanonical or has a stale hash".into(),
        );
    }
    let foundation = load_schedule_foundation(foundation_root)?;
    let binding = foundation
        .scheduled_profiles
        .get(&value.source.row_id)
        .ok_or("schedule authority: scheduled source row is not in the checked-in foundation")?;
    if value.profile_policy_bundle_sha256 != foundation.profile_policy_bundle_sha256 {
        return Err("schedule authority: scheduled source profile-policy bundle drifted".into());
    }
    validate_foundation_binding(
        binding,
        FoundationBindingObservationV1 {
            source: &value.source,
            characterization_profile: &value.characterization_profile,
            requested_identity: &value.requested_identity,
            expected_effective_identity: &value.expected_effective_identity,
            caps: &value.caps,
            config_template_sha256: &value.config_template_sha256,
            generated_config_sha256: &value.case_execution.input.bindings.generated_config_sha256,
        },
    )
}

pub(super) fn generate_scheduled_execution_source(
    foundation_root: &Path,
    case_id: &str,
    case_execution: CaseExecutionFingerprintRecordV1,
    admission_attempt: AdmissionAttemptFingerprintRecordV1,
    authority: AdmissionAuthorityV1,
    trigger: TriggerKindV1,
) -> Result<ScheduledExecutionSourceV1, BoxError> {
    let foundation = load_schedule_foundation(foundation_root)?;
    let binding = foundation
        .scheduled_profiles
        .get(case_id)
        .ok_or("schedule authority: scheduled source row is not in the checked-in foundation")?;
    let value = seal_scheduled_execution_source(ScheduledExecutionSourceV1 {
        schema_version: 1,
        source_sha256: ZERO_SHA256.into(),
        source: binding.source.clone(),
        profile_policy_bundle_sha256: foundation.profile_policy_bundle_sha256,
        characterization_profile: binding.characterization_profile.clone(),
        caps: case_execution.input.actual_caps.clone(),
        case_execution,
        admission_attempt,
        authority,
        trigger,
        config_template_sha256: binding.config_template_sha256.clone(),
        requested_identity: binding.requested_identity.clone(),
        expected_effective_identity: binding.expected_effective_identity.clone(),
        retry_cap: 0,
        fallback_cap: 0,
    })?;
    validate_scheduled_execution_source(foundation_root, &value)?;
    Ok(value)
}

pub(super) fn seal_claimed_support_characterization_source(
    mut value: ClaimedSupportCharacterizationSourceV1,
) -> Result<ClaimedSupportCharacterizationSourceV1, BoxError> {
    value.source_sha256 = ZERO_SHA256.into();
    value.validate()?;
    value.source_sha256 = claimed_support_source_sha256(&value)?;
    value.validate()?;
    Ok(value)
}

pub(super) fn validate_claimed_support_characterization_source(
    foundation_root: &Path,
    value: &ClaimedSupportCharacterizationSourceV1,
) -> Result<(), BoxError> {
    value.validate()?;
    if value.source_sha256 != claimed_support_source_sha256(value)? {
        return Err(
            "schedule authority: claimed-support source is noncanonical or has a stale hash".into(),
        );
    }
    let foundation = load_schedule_foundation(foundation_root)?;
    let binding = foundation
        .claimed_support_profiles
        .get(&value.source.row_id)
        .ok_or("schedule authority: claimed-support row is not in the checked-in foundation")?;
    if value.production_manifest_sha256 != binding.source.source_sha256
        || value.pinned_config_sha256 != binding.exact_config_sha256
        || value.profile_policy_bundle_sha256 != foundation.profile_policy_bundle_sha256
    {
        return Err("schedule authority: claimed-support source exact pins drifted".into());
    }
    validate_foundation_binding(
        binding,
        FoundationBindingObservationV1 {
            source: &value.source,
            characterization_profile: &value.characterization_profile,
            requested_identity: &value.requested_identity,
            expected_effective_identity: &value.expected_effective_identity,
            caps: &value.caps,
            config_template_sha256: &value.config_template_sha256,
            generated_config_sha256: &value
                .characterization_execution
                .input
                .bindings
                .generated_config_sha256,
        },
    )
}

pub(super) fn generate_claimed_support_characterization_source(
    foundation_root: &Path,
    case_id: &str,
    characterization_execution: CaseExecutionFingerprintRecordV1,
    admission_attempt: AdmissionAttemptFingerprintRecordV1,
    authority: AdmissionAuthorityV1,
) -> Result<ClaimedSupportCharacterizationSourceV1, BoxError> {
    let foundation = load_schedule_foundation(foundation_root)?;
    let binding = foundation
        .claimed_support_profiles
        .get(case_id)
        .ok_or("schedule authority: claimed-support row is not in the checked-in foundation")?;
    let value =
        seal_claimed_support_characterization_source(ClaimedSupportCharacterizationSourceV1 {
            schema_version: 1,
            source_sha256: ZERO_SHA256.into(),
            source: binding.source.clone(),
            production_manifest_sha256: binding.source.source_sha256.clone(),
            profile_policy_bundle_sha256: foundation.profile_policy_bundle_sha256,
            characterization_profile: binding.characterization_profile.clone(),
            caps: characterization_execution.input.actual_caps.clone(),
            characterization_execution,
            admission_attempt,
            authority,
            trigger: TriggerKindV1::ManualCharacterization,
            pinned_config_sha256: binding.exact_config_sha256.clone(),
            config_template_sha256: binding.config_template_sha256.clone(),
            requested_identity: binding.requested_identity.clone(),
            expected_effective_identity: binding.expected_effective_identity.clone(),
        })?;
    validate_claimed_support_characterization_source(foundation_root, &value)?;
    Ok(value)
}

const MAX_MANUAL_ADMISSION_LIFETIME_MS: i64 = 15 * 60 * 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ManualAdmissionOriginV1 {
    DirectLocalCompatibilityCli,
    Serve,
    A2a,
    Timer,
    Watcher,
}

#[derive(Clone, Debug)]
pub(super) struct ManualAdmissionBindingsV1 {
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) scheduler_binary_sha256: String,
    pub(super) input_source_sha256: String,
    pub(super) characterization_profile: crate::compatibility_schedule_schema::FingerprintV1,
    pub(super) case_execution: crate::compatibility_schedule_schema::FingerprintV1,
    pub(super) evidence_purpose: EvidencePurposeV1,
    pub(super) freshness_bucket: String,
    pub(super) caps: EffectCapsV1,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) issued_at_ms: i64,
    pub(super) expires_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SealedManualAdmissionV1 {
    pub(super) record: ManualAdmissionV1,
    pub(super) authority: AdmissionAuthorityV1,
}

pub(super) trait ManualNonceSource {
    fn fill(&self, output: &mut [u8]) -> Result<(), BoxError>;
}

pub(super) struct SystemManualNonceSource;

impl ManualNonceSource for SystemManualNonceSource {
    fn fill(&self, output: &mut [u8]) -> Result<(), BoxError> {
        use ring::rand::SecureRandom as _;

        ring::rand::SystemRandom::new()
            .fill(output)
            .map_err(|_| "schedule authority: secure manual nonce generation failed".into())
    }
}

pub(super) fn manual_admission_sha256(value: &ManualAdmissionV1) -> Result<String, BoxError> {
    value.validate()?;
    let mut canonical = value.clone();
    canonical.allowed_effects.sort();
    if &canonical != value {
        return Err("schedule authority: manual admission effects are noncanonical".into());
    }
    authority_hash("manual-admission", value)
}

pub(super) fn validate_sealed_manual_admission(
    value: &SealedManualAdmissionV1,
) -> Result<(), BoxError> {
    let expected_sha256 = manual_admission_sha256(&value.record)?;
    let lifetime_ms = value
        .record
        .expires_at_ms
        .checked_sub(value.record.issued_at_ms)
        .ok_or("schedule authority: manual admission lifetime overflow")?;
    if lifetime_ms > MAX_MANUAL_ADMISSION_LIFETIME_MS {
        return Err("schedule authority: manual admission exceeds the one-run lifetime".into());
    }
    match &value.authority {
        AdmissionAuthorityV1::ManualAcknowledgement(authority)
            if authority.manual_admission_sha256 == expected_sha256
                && authority.request_nonce == value.record.request_nonce =>
        {
            Ok(())
        }
        _ => Err("schedule authority: manual admission authority binding mismatch".into()),
    }
}

pub(super) fn derive_manual_admission<N: ManualNonceSource + ?Sized>(
    origin: ManualAdmissionOriginV1,
    acknowledged_billable: bool,
    caller_request_nonce: Option<&str>,
    nonce_source: &N,
    bindings: ManualAdmissionBindingsV1,
) -> Result<SealedManualAdmissionV1, BoxError> {
    if origin != ManualAdmissionOriginV1::DirectLocalCompatibilityCli {
        return Err("schedule authority: manual admission requires the direct local CLI".into());
    }
    if !acknowledged_billable {
        return Err(
            "schedule authority: manual admission requires explicit billable acknowledgement"
                .into(),
        );
    }
    if caller_request_nonce.is_some() {
        return Err("schedule authority: a caller cannot supply the manual request nonce".into());
    }
    let mut nonce_bytes = [0_u8; 32];
    nonce_source.fill(&mut nonce_bytes)?;
    let request_nonce = local_file::sha256_hex(&nonce_bytes);
    let mut allowed_effects = bindings.allowed_effects;
    allowed_effects.sort();
    let record = ManualAdmissionV1 {
        schema_version: 1,
        request_nonce: request_nonce.clone(),
        operator: bindings.operator,
        environment_owner: bindings.environment_owner,
        scheduler_binary_sha256: bindings.scheduler_binary_sha256,
        input_source_sha256: bindings.input_source_sha256,
        characterization_profile: bindings.characterization_profile,
        case_execution: bindings.case_execution,
        evidence_purpose: bindings.evidence_purpose,
        freshness_bucket: bindings.freshness_bucket,
        source: ManualAdmissionSourceV1::DirectLocalCli,
        command: ManualCommandV1::CompatibilityRun,
        caps: bindings.caps,
        allowed_effects,
        retry_cap: 0,
        fallback_cap: 0,
        acknowledged_billable: true,
        issued_at_ms: bindings.issued_at_ms,
        expires_at_ms: bindings.expires_at_ms,
    };
    let authority = AdmissionAuthorityV1::ManualAcknowledgement(ManualAcknowledgementAuthorityV1 {
        manual_admission_sha256: manual_admission_sha256(&record)?,
        request_nonce,
    });
    let value = SealedManualAdmissionV1 { record, authority };
    validate_sealed_manual_admission(&value)?;
    Ok(value)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OneShotLifecyclePhaseV1 {
    Available,
    ConsumedUnreconciled {
        admission_commit_sha256: String,
        consumed_at_ms: i64,
    },
    Reconciled {
        admission_commit_sha256: String,
        terminal_record_sha256: String,
        consumed_at_ms: i64,
        reconciled_at_ms: i64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct OneShotLifecycleV1 {
    pub(super) authorization_id: String,
    pub(super) authorization_sha256: String,
    pub(super) entry_id: String,
    pub(super) entry_sha256: String,
    pub(super) characterization_profile_sha256: String,
    pub(super) revocation_generation: u64,
    pub(super) phase: OneShotLifecyclePhaseV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ManualAdmissionConsumptionV1 {
    pub(super) record: ManualAdmissionV1,
    pub(super) authority: ManualAcknowledgementAuthorityV1,
    pub(super) admission_commit_sha256: String,
    pub(super) consumed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorityStateModelV1 {
    #[serde(default)]
    pub(super) authorizations: BTreeMap<String, CharacterizationAuthorizationV1>,
    #[serde(default)]
    pub(super) one_shots: BTreeMap<String, OneShotLifecycleV1>,
    #[serde(default)]
    pub(super) grants: BTreeMap<String, ProviderEffectGrantV1>,
    #[serde(default)]
    pub(super) grant_revocations: BTreeMap<String, u64>,
    #[serde(default)]
    pub(super) storage_consents: BTreeMap<String, StorageConsentV1>,
    #[serde(default)]
    pub(super) storage_revocations: BTreeMap<String, u64>,
    #[serde(default)]
    pub(super) profile_index: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) manual_admissions: BTreeMap<String, ManualAdmissionConsumptionV1>,
}

impl AuthorityStateModelV1 {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn static_entry(&self, entry_id: &str) -> Option<&OneShotCharacterizationEntryV1> {
        let runtime = self.one_shots.get(entry_id)?;
        self.authorizations
            .get(&runtime.authorization_id)?
            .entries
            .iter()
            .find(|entry| entry.entry_id == entry_id)
    }

    fn derived_profile_index(&self) -> Result<BTreeMap<String, String>, BoxError> {
        let mut index = BTreeMap::new();
        for (entry_id, runtime) in &self.one_shots {
            let entry = self.static_entry(entry_id).ok_or_else(|| {
                format!("schedule authority: lifecycle {entry_id:?} has no immutable entry")
            })?;
            let blocks_profile = match runtime.phase {
                OneShotLifecyclePhaseV1::Available => {
                    runtime.revocation_generation == entry.revocation_generation
                }
                OneShotLifecyclePhaseV1::ConsumedUnreconciled { .. } => true,
                OneShotLifecyclePhaseV1::Reconciled { .. } => false,
            };
            if blocks_profile
                && index
                    .insert(
                        entry.characterization_profile.sha256.clone(),
                        entry_id.clone(),
                    )
                    .is_some()
            {
                return Err(
                    "schedule authority: more than one live one-shot entry targets a profile"
                        .into(),
                );
            }
        }
        Ok(index)
    }

    pub(super) fn validate(&self) -> Result<(), BoxError> {
        let mut entry_ids = BTreeSet::new();
        let mut nonces = BTreeSet::new();
        for (authorization_id, authorization) in &self.authorizations {
            if authorization_id != &authorization.authorization_id {
                return Err("schedule authority: authorization map key mismatch".into());
            }
            validate_sealed_characterization_authorization(authorization)?;
            for entry in &authorization.entries {
                if !entry_ids.insert(entry.entry_id.as_str())
                    || !nonces.insert(entry.consumption_nonce.as_str())
                {
                    return Err(
                        "schedule authority: one-shot ids and nonces must be globally unique"
                            .into(),
                    );
                }
                let runtime = self.one_shots.get(&entry.entry_id).ok_or_else(|| {
                    format!(
                        "schedule authority: immutable entry {:?} has no lifecycle",
                        entry.entry_id
                    )
                })?;
                if runtime.authorization_id != authorization.authorization_id
                    || runtime.authorization_sha256 != authorization.authorization_sha256
                    || runtime.entry_id != entry.entry_id
                    || runtime.entry_sha256 != entry.entry_sha256
                    || runtime.characterization_profile_sha256
                        != entry.characterization_profile.sha256
                    || runtime.revocation_generation < entry.revocation_generation
                {
                    return Err("schedule authority: one-shot lifecycle binding mismatch".into());
                }
                match &runtime.phase {
                    OneShotLifecyclePhaseV1::Available => {}
                    OneShotLifecyclePhaseV1::ConsumedUnreconciled {
                        admission_commit_sha256,
                        consumed_at_ms,
                    } => {
                        require_sha256("one-shot admission commit", admission_commit_sha256)?;
                        if *consumed_at_ms <= 0 {
                            return Err(
                                "schedule authority: one-shot consumption time must be positive"
                                    .into(),
                            );
                        }
                    }
                    OneShotLifecyclePhaseV1::Reconciled {
                        admission_commit_sha256,
                        terminal_record_sha256,
                        consumed_at_ms,
                        reconciled_at_ms,
                    } => {
                        require_sha256("one-shot admission commit", admission_commit_sha256)?;
                        require_sha256("one-shot terminal record", terminal_record_sha256)?;
                        if *consumed_at_ms <= 0 || *reconciled_at_ms < *consumed_at_ms {
                            return Err(
                                "schedule authority: one-shot reconciliation times are invalid"
                                    .into(),
                            );
                        }
                    }
                }
            }
        }
        if entry_ids.len() != self.one_shots.len() {
            return Err("schedule authority: lifecycle contains an unknown one-shot entry".into());
        }
        for (grant_id, grant) in &self.grants {
            if grant_id != &grant.grant_id {
                return Err("schedule authority: grant map key mismatch".into());
            }
            validate_sealed_provider_effect_grant(grant)?;
            if self.grant_revocations.get(grant_id).copied() < Some(grant.revocation_generation) {
                return Err(
                    "schedule authority: grant revocation state is missing or stale".into(),
                );
            }
        }
        if self.grants.len() != self.grant_revocations.len() {
            return Err("schedule authority: grant revocation state has an unknown id".into());
        }
        if self
            .grants
            .iter()
            .filter(|(id, grant)| {
                self.grant_revocations.get(*id).copied() == Some(grant.revocation_generation)
            })
            .count()
            > 1
        {
            return Err("schedule authority: more than one standing grant is active".into());
        }
        for (consent_id, consent) in &self.storage_consents {
            if consent_id != &consent.consent_id {
                return Err("schedule authority: storage-consent map key mismatch".into());
            }
            validate_sealed_storage_consent(consent)?;
            if self.storage_revocations.get(consent_id).copied()
                < Some(consent.revocation_generation)
            {
                return Err(
                    "schedule authority: storage-consent revocation state is missing or stale"
                        .into(),
                );
            }
        }
        if self.storage_consents.len() != self.storage_revocations.len() {
            return Err("schedule authority: storage revocation state has an unknown id".into());
        }
        if self
            .storage_consents
            .iter()
            .filter(|(id, consent)| {
                self.storage_revocations.get(*id).copied() == Some(consent.revocation_generation)
            })
            .count()
            > 1
        {
            return Err("schedule authority: more than one storage consent is active".into());
        }
        for (request_nonce, consumption) in &self.manual_admissions {
            if !nonces.insert(request_nonce.as_str()) {
                return Err("schedule authority: authority nonces must be globally unique".into());
            }
            let sealed = SealedManualAdmissionV1 {
                record: consumption.record.clone(),
                authority: AdmissionAuthorityV1::ManualAcknowledgement(
                    consumption.authority.clone(),
                ),
            };
            validate_sealed_manual_admission(&sealed)?;
            require_sha256(
                "manual admission commit",
                &consumption.admission_commit_sha256,
            )?;
            if request_nonce != &consumption.record.request_nonce
                || consumption.consumed_at_ms < consumption.record.issued_at_ms
                || consumption.consumed_at_ms > consumption.record.expires_at_ms
            {
                return Err("schedule authority: manual admission consumption is invalid".into());
            }
        }
        if self.profile_index != self.derived_profile_index()? {
            return Err(
                "schedule authority: profile index diverges from authoritative state".into(),
            );
        }
        Ok(())
    }

    pub(super) fn rebuild_profile_index(&mut self) -> Result<(), BoxError> {
        self.profile_index = self.derived_profile_index()?;
        Ok(())
    }

    pub(super) fn issue_authorization(
        &mut self,
        authorization: CharacterizationAuthorizationV1,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.issue_authorization_in_place(authorization)?;
        *self = candidate;
        Ok(())
    }

    fn issue_authorization_in_place(
        &mut self,
        authorization: CharacterizationAuthorizationV1,
    ) -> Result<(), BoxError> {
        self.validate()?;
        validate_sealed_characterization_authorization(&authorization)?;
        if self
            .authorizations
            .contains_key(&authorization.authorization_id)
        {
            return Err("schedule authority: authorization id already exists".into());
        }
        let existing_ids = self.one_shots.keys().cloned().collect::<BTreeSet<_>>();
        let mut proposed_nonces = self
            .authorizations
            .values()
            .flat_map(|record| {
                record
                    .entries
                    .iter()
                    .map(|entry| entry.consumption_nonce.clone())
            })
            .collect::<BTreeSet<_>>();
        proposed_nonces.extend(self.manual_admissions.keys().cloned());
        let mut proposed_profiles = self.profile_index.clone();
        for entry in &authorization.entries {
            if existing_ids.contains(&entry.entry_id)
                || !proposed_nonces.insert(entry.consumption_nonce.clone())
            {
                return Err("schedule authority: one-shot id or nonce already exists".into());
            }
            if proposed_profiles
                .insert(
                    entry.characterization_profile.sha256.clone(),
                    entry.entry_id.clone(),
                )
                .is_some()
            {
                return Err(
                    "schedule authority: profile already has a live or unreconciled one-shot entry"
                        .into(),
                );
            }
            self.validate_reissue(entry)?;
        }
        for entry in &authorization.entries {
            self.one_shots.insert(
                entry.entry_id.clone(),
                OneShotLifecycleV1 {
                    authorization_id: authorization.authorization_id.clone(),
                    authorization_sha256: authorization.authorization_sha256.clone(),
                    entry_id: entry.entry_id.clone(),
                    entry_sha256: entry.entry_sha256.clone(),
                    characterization_profile_sha256: entry.characterization_profile.sha256.clone(),
                    revocation_generation: entry.revocation_generation,
                    phase: OneShotLifecyclePhaseV1::Available,
                },
            );
        }
        self.authorizations
            .insert(authorization.authorization_id.clone(), authorization);
        self.profile_index = proposed_profiles;
        self.validate()
    }

    fn validate_reissue(&self, entry: &OneShotCharacterizationEntryV1) -> Result<(), BoxError> {
        let same_profile = self
            .one_shots
            .iter()
            .filter(|(_, runtime)| {
                runtime.characterization_profile_sha256 == entry.characterization_profile.sha256
            })
            .collect::<Vec<_>>();
        if same_profile.is_empty() {
            if entry.generation != 1
                || !matches!(entry.prior_entry, OptionalRecordRefV1::Absent)
                || !matches!(entry.reissue_reason, OptionalTextV1::Absent)
            {
                return Err(
                    "schedule authority: first one-shot entry must be generation 1 without reissue fields"
                        .into(),
                );
            }
            return Ok(());
        }
        let referenced = self
            .authorizations
            .values()
            .flat_map(|authorization| authorization.entries.iter())
            .filter_map(|candidate| match &candidate.prior_entry {
                OptionalRecordRefV1::Record { id, .. }
                    if candidate.characterization_profile == entry.characterization_profile =>
                {
                    Some(id.as_str())
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let leaves = same_profile
            .iter()
            .filter(|(id, _)| !referenced.contains(id.as_str()))
            .collect::<Vec<_>>();
        if leaves.len() != 1 {
            return Err("schedule authority: one-shot reissue history is branched".into());
        }
        let (prior_id, prior_runtime) = leaves[0];
        let prior_entry = self
            .static_entry(prior_id)
            .ok_or("schedule authority: reissue predecessor is missing")?;
        let prior_is_terminal =
            matches!(
                prior_runtime.phase,
                OneShotLifecyclePhaseV1::Reconciled { .. }
            ) || (matches!(prior_runtime.phase, OneShotLifecyclePhaseV1::Available)
                && prior_runtime.revocation_generation > prior_entry.revocation_generation);
        let prior_matches = matches!(
            &entry.prior_entry,
            OptionalRecordRefV1::Record { id, sha256 }
                if id == prior_id.as_str() && sha256 == &prior_runtime.entry_sha256
        );
        if !prior_is_terminal
            || !prior_matches
            || entry.generation != prior_entry.generation.saturating_add(1)
            || !matches!(entry.reissue_reason, OptionalTextV1::Text { .. })
        {
            return Err(
                "schedule authority: reissue must name the sole terminal predecessor and next generation"
                    .into(),
            );
        }
        Ok(())
    }

    pub(super) fn install_grant(&mut self, grant: ProviderEffectGrantV1) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.install_grant_in_place(grant)?;
        *self = candidate;
        Ok(())
    }

    fn install_grant_in_place(&mut self, grant: ProviderEffectGrantV1) -> Result<(), BoxError> {
        self.validate()?;
        validate_sealed_provider_effect_grant(&grant)?;
        if self.grants.contains_key(&grant.grant_id) {
            return Err("schedule authority: provider grant id already exists".into());
        }
        self.grant_revocations
            .insert(grant.grant_id.clone(), grant.revocation_generation);
        self.grants.insert(grant.grant_id.clone(), grant);
        self.validate()
    }

    pub(super) fn install_storage_consent(
        &mut self,
        consent: StorageConsentV1,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.install_storage_consent_in_place(consent)?;
        *self = candidate;
        Ok(())
    }

    fn install_storage_consent_in_place(
        &mut self,
        consent: StorageConsentV1,
    ) -> Result<(), BoxError> {
        self.validate()?;
        validate_sealed_storage_consent(&consent)?;
        if self.storage_consents.contains_key(&consent.consent_id) {
            return Err("schedule authority: storage consent id already exists".into());
        }
        self.storage_revocations
            .insert(consent.consent_id.clone(), consent.revocation_generation);
        self.storage_consents
            .insert(consent.consent_id.clone(), consent);
        self.validate()
    }

    pub(super) fn consume_one_shot(
        &mut self,
        entry_id: &str,
        admission_commit_sha256: &str,
        consumed_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.consume_one_shot_in_place(entry_id, admission_commit_sha256, consumed_at_ms)?;
        *self = candidate;
        Ok(())
    }

    fn consume_one_shot_in_place(
        &mut self,
        entry_id: &str,
        admission_commit_sha256: &str,
        consumed_at_ms: i64,
    ) -> Result<(), BoxError> {
        self.validate()?;
        require_sha256("one-shot admission commit", admission_commit_sha256)?;
        let static_revocation = self
            .static_entry(entry_id)
            .ok_or("schedule authority: one-shot entry does not exist")?
            .revocation_generation;
        let runtime = self
            .one_shots
            .get_mut(entry_id)
            .ok_or("schedule authority: one-shot lifecycle does not exist")?;
        if consumed_at_ms <= 0
            || runtime.revocation_generation != static_revocation
            || !matches!(runtime.phase, OneShotLifecyclePhaseV1::Available)
        {
            return Err("schedule authority: one-shot entry is not available".into());
        }
        runtime.phase = OneShotLifecyclePhaseV1::ConsumedUnreconciled {
            admission_commit_sha256: admission_commit_sha256.into(),
            consumed_at_ms,
        };
        self.validate()
    }

    pub(super) fn consume_manual_admission(
        &mut self,
        value: SealedManualAdmissionV1,
        admission_commit_sha256: &str,
        consumed_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.consume_manual_admission_in_place(
            value,
            admission_commit_sha256,
            consumed_at_ms,
        )?;
        *self = candidate;
        Ok(())
    }

    fn consume_manual_admission_in_place(
        &mut self,
        value: SealedManualAdmissionV1,
        admission_commit_sha256: &str,
        consumed_at_ms: i64,
    ) -> Result<(), BoxError> {
        self.validate()?;
        validate_sealed_manual_admission(&value)?;
        require_sha256("manual admission commit", admission_commit_sha256)?;
        let authority = match value.authority {
            AdmissionAuthorityV1::ManualAcknowledgement(value) => value,
            _ => {
                return Err("schedule authority: manual admission has persistent authority".into())
            }
        };
        if self
            .manual_admissions
            .contains_key(&value.record.request_nonce)
            || consumed_at_ms < value.record.issued_at_ms
            || consumed_at_ms > value.record.expires_at_ms
        {
            return Err(
                "schedule authority: manual admission is expired or already consumed".into(),
            );
        }
        self.manual_admissions.insert(
            value.record.request_nonce.clone(),
            ManualAdmissionConsumptionV1 {
                record: value.record,
                authority,
                admission_commit_sha256: admission_commit_sha256.into(),
                consumed_at_ms,
            },
        );
        self.validate()
    }

    pub(super) fn reconcile_one_shot(
        &mut self,
        entry_id: &str,
        admission_commit_sha256: &str,
        terminal_record_sha256: &str,
        reconciled_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.reconcile_one_shot_in_place(
            entry_id,
            admission_commit_sha256,
            terminal_record_sha256,
            reconciled_at_ms,
        )?;
        *self = candidate;
        Ok(())
    }

    fn reconcile_one_shot_in_place(
        &mut self,
        entry_id: &str,
        admission_commit_sha256: &str,
        terminal_record_sha256: &str,
        reconciled_at_ms: i64,
    ) -> Result<(), BoxError> {
        self.validate()?;
        require_sha256("one-shot admission commit", admission_commit_sha256)?;
        require_sha256("one-shot terminal record", terminal_record_sha256)?;
        let runtime = self
            .one_shots
            .get_mut(entry_id)
            .ok_or("schedule authority: one-shot lifecycle does not exist")?;
        let consumed_at_ms = match &runtime.phase {
            OneShotLifecyclePhaseV1::ConsumedUnreconciled {
                admission_commit_sha256: observed,
                consumed_at_ms,
            } if observed == admission_commit_sha256 => *consumed_at_ms,
            _ => return Err("schedule authority: one-shot entry is not consumable history".into()),
        };
        if reconciled_at_ms < consumed_at_ms {
            return Err("schedule authority: reconciliation predates consumption".into());
        }
        runtime.phase = OneShotLifecyclePhaseV1::Reconciled {
            admission_commit_sha256: admission_commit_sha256.into(),
            terminal_record_sha256: terminal_record_sha256.into(),
            consumed_at_ms,
            reconciled_at_ms,
        };
        self.rebuild_profile_index()?;
        self.validate()
    }

    pub(super) fn rollback_provider_authority(&mut self) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.rollback_provider_authority_in_place()?;
        *self = candidate;
        Ok(())
    }

    fn rollback_provider_authority_in_place(&mut self) -> Result<(), BoxError> {
        self.validate()?;
        for generation in self.grant_revocations.values_mut() {
            *generation = generation
                .checked_add(1)
                .ok_or("schedule authority: grant revocation generation overflow")?;
        }
        for (entry_id, runtime) in &mut self.one_shots {
            if matches!(
                runtime.phase,
                OneShotLifecyclePhaseV1::Available
                    | OneShotLifecyclePhaseV1::ConsumedUnreconciled { .. }
            ) {
                runtime.revocation_generation = runtime
                    .revocation_generation
                    .checked_add(1)
                    .ok_or("schedule authority: entry revocation generation overflow")?;
                if matches!(runtime.phase, OneShotLifecyclePhaseV1::Available) {
                    self.profile_index
                        .retain(|_, indexed_entry| indexed_entry != entry_id);
                }
            }
        }
        self.validate()
    }

    pub(super) fn revoke_storage_consent(&mut self, consent_id: &str) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.revoke_storage_consent_in_place(consent_id)?;
        *self = candidate;
        Ok(())
    }

    fn revoke_storage_consent_in_place(&mut self, consent_id: &str) -> Result<(), BoxError> {
        self.validate()?;
        let generation = self
            .storage_revocations
            .get_mut(consent_id)
            .ok_or("schedule authority: storage consent does not exist")?;
        *generation = generation
            .checked_add(1)
            .ok_or("schedule authority: storage revocation generation overflow")?;
        self.validate()
    }
}

fn require_sha256(label: &str, value: &str) -> Result<(), BoxError> {
    if !local_file::valid_sha256(value) || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(format!("schedule authority: {label} is not lowercase SHA-256").into());
    }
    Ok(())
}

fn validate_requested_effects(values: &[EffectClassV1]) -> Result<(), BoxError> {
    let mut canonical = values.to_vec();
    canonical.sort();
    canonical.dedup();
    if values.is_empty() || canonical.as_slice() != values {
        return Err(
            "schedule authority: requested effects must be nonempty, unique, and sorted".into(),
        );
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(super) struct AuthorityEnvironmentV1 {
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) host_identity_sha256: String,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) scheduler_binary_sha256: String,
    pub(super) price_snapshot_sha256: String,
    pub(super) legacy_inventory_sha256: String,
    pub(super) now_ms: i64,
    pub(super) terminal_deadline_ms: i64,
}

fn validate_window(
    environment: &AuthorityEnvironmentV1,
    not_before_ms: i64,
    expires_at_ms: i64,
) -> Result<(), BoxError> {
    if environment.now_ms < not_before_ms
        || environment.terminal_deadline_ms < environment.now_ms
        || environment.terminal_deadline_ms > expires_at_ms
    {
        return Err(
            "schedule authority: authority is not current through the terminal deadline".into(),
        );
    }
    Ok(())
}

fn validate_common_authorization(
    authorization: &CharacterizationAuthorizationV1,
    environment: &AuthorityEnvironmentV1,
) -> Result<(), BoxError> {
    validate_sealed_characterization_authorization(authorization)?;
    if authorization.operator != environment.operator
        || authorization.environment_owner != environment.environment_owner
        || authorization.host_identity_sha256 != environment.host_identity_sha256
        || authorization.profile_policy_bundle_sha256 != environment.profile_policy_bundle_sha256
        || authorization.scheduler_binary_sha256 != environment.scheduler_binary_sha256
        || authorization.price_snapshot_sha256 != environment.price_snapshot_sha256
        || authorization.legacy_inventory_sha256 != environment.legacy_inventory_sha256
        || authorization.issued_at_ms > environment.now_ms
    {
        return Err("schedule authority: characterization authorization binding mismatch".into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(super) struct CharacterizationAdmissionRequestV1 {
    pub(super) entry_id: String,
    pub(super) source: ProfileSourceRefV1,
    pub(super) characterization_profile_sha256: String,
    pub(super) characterization_execution_sha256: String,
    pub(super) proposed_effective_identity:
        crate::compatibility_schedule_schema::EffectiveIdentityV1,
    pub(super) provider_family: String,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) caps: EffectCapsV1,
    pub(super) command: String,
    pub(super) characterization_already_exists: bool,
}

pub(super) fn select_characterization_authority(
    state: &AuthorityStateModelV1,
    authorization_id: &str,
    environment: &AuthorityEnvironmentV1,
    request: &CharacterizationAdmissionRequestV1,
) -> Result<AdmissionAuthorityV1, BoxError> {
    state.validate()?;
    let authorization = state
        .authorizations
        .get(authorization_id)
        .ok_or("schedule authority: characterization authorization does not exist")?;
    validate_common_authorization(authorization, environment)?;
    let entry = authorization
        .entries
        .iter()
        .find(|entry| entry.entry_id == request.entry_id)
        .ok_or("schedule authority: one-shot entry does not exist")?;
    let runtime = state
        .one_shots
        .get(&entry.entry_id)
        .ok_or("schedule authority: one-shot lifecycle does not exist")?;
    validate_window(environment, entry.not_before_ms, entry.expires_at_ms)?;
    if request.characterization_already_exists
        || runtime.revocation_generation != entry.revocation_generation
        || !matches!(runtime.phase, OneShotLifecyclePhaseV1::Available)
        || entry.source != request.source
        || entry.characterization_profile.sha256 != request.characterization_profile_sha256
        || entry.characterization_execution.sha256 != request.characterization_execution_sha256
        || entry.proposed_effective_identity != request.proposed_effective_identity
        || entry.provider_family != request.provider_family
        || entry.allowed_effects != request.allowed_effects
        || entry.caps != request.caps
        || entry.command != request.command
    {
        return Err(
            "schedule authority: one-shot request does not match an available entry".into(),
        );
    }
    Ok(AdmissionAuthorityV1::CharacterizationOnce(
        CharacterizationOnceAuthorityV1 {
            batch_authorization_id: authorization.authorization_id.clone(),
            batch_authorization_sha256: authorization.authorization_sha256.clone(),
            entry_id: entry.entry_id.clone(),
            generation: entry.generation,
            entry_sha256: entry.entry_sha256.clone(),
            consumption_nonce: entry.consumption_nonce.clone(),
        },
    ))
}

#[derive(Clone, Debug)]
pub(super) struct StandingAdmissionRequestV1 {
    pub(super) trigger: TriggerKindV1,
    pub(super) case_id: String,
    pub(super) provider_family: String,
    pub(super) source: ProfileSourceRefV1,
    pub(super) characterization_profile_sha256: String,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) caps: EffectCapsV1,
    pub(super) launchd: Option<LaunchdBindingV1>,
    pub(super) characterization: CharacterizationRecordV1,
}

pub(super) fn select_standing_grant(
    state: &AuthorityStateModelV1,
    grant_id: &str,
    environment: &AuthorityEnvironmentV1,
    request: &StandingAdmissionRequestV1,
) -> Result<AdmissionAuthorityV1, BoxError> {
    state.validate()?;
    let grant = state
        .grants
        .get(grant_id)
        .ok_or("schedule authority: provider grant does not exist")?;
    validate_sealed_provider_effect_grant(grant)?;
    validate_window(environment, grant.not_before_ms, grant.expires_at_ms)?;
    request.caps.validate("standing admission caps")?;
    validate_requested_effects(&request.allowed_effects)?;
    if grant.operator != environment.operator
        || grant.environment_owner != environment.environment_owner
        || grant.host_identity_sha256 != environment.host_identity_sha256
        || grant.profile_policy_bundle_sha256 != environment.profile_policy_bundle_sha256
        || grant.scheduler_binary_sha256 != environment.scheduler_binary_sha256
        || grant.price_snapshot_sha256 != environment.price_snapshot_sha256
        || grant.legacy_inventory_sha256 != environment.legacy_inventory_sha256
        || environment.now_ms < grant.price_snapshot_observed_at_ms
        || environment.now_ms > grant.price_snapshot_valid_until_ms
        || state.grant_revocations.get(grant_id).copied() != Some(grant.revocation_generation)
        || !grant.triggers.contains(&request.trigger)
        || !grant.case_ids.contains(&request.case_id)
        || !grant.provider_families.contains(&request.provider_family)
        || !request
            .allowed_effects
            .iter()
            .all(|effect| grant.allowed_effects.contains(effect))
    {
        return Err("schedule authority: standing grant binding mismatch or revoked".into());
    }
    request
        .caps
        .within(&grant.per_run_caps, "standing admission caps")?;
    let profile = grant
        .profiles
        .iter()
        .find(|profile| profile.case_id == request.case_id)
        .ok_or("schedule authority: standing grant has no case profile")?;
    request
        .caps
        .within(&profile.caps, "standing characterized caps")?;
    request.characterization.validate()?;
    let characterization_sha256 = characterization_record_sha256(&request.characterization)?;
    if profile.provider_family != request.provider_family
        || profile.source != request.source
        || profile.characterization_profile.sha256 != request.characterization_profile_sha256
        || profile.characterization_id != request.characterization.characterization_id
        || profile.characterization_sha256 != characterization_sha256
        || request.characterization.source != request.source
        || request.characterization.profile_policy_bundle_sha256
            != environment.profile_policy_bundle_sha256
        || request.characterization.characterization_profile.sha256
            != request.characterization_profile_sha256
        || request.characterization.observed_effective_identity != profile.effective_identity
        || request.characterization.expected_effective_identity != profile.effective_identity
        || request.characterization.outcome
            == CharacterizationOutcomeV1::CharacterizationInconclusive
        || request.characterization.terminal_at_ms > environment.now_ms
    {
        return Err(
            "schedule authority: standing grant lacks its exact completed characterization".into(),
        );
    }
    let expected_launchd = grant
        .launchd
        .iter()
        .find(|binding| binding.trigger == request.trigger);
    let launchd_matches = match request.trigger {
        TriggerKindV1::Daily | TriggerKindV1::TestMerge => {
            request.launchd.as_ref() == expected_launchd
        }
        TriggerKindV1::ScheduledMain => request.launchd.is_none() && expected_launchd.is_none(),
        TriggerKindV1::ManualCharacterization | TriggerKindV1::ManualCompatibility => false,
    };
    if !launchd_matches {
        return Err("schedule authority: launchd binding does not match the trigger".into());
    }
    Ok(AdmissionAuthorityV1::StandingGrant(
        StandingGrantAuthorityV1 {
            grant_id: grant.grant_id.clone(),
            generation: grant.generation,
            grant_sha256: grant.grant_sha256.clone(),
            characterization_id: profile.characterization_id.clone(),
            characterization_sha256: profile.characterization_sha256.clone(),
        },
    ))
}

#[derive(Clone, Debug)]
pub(super) struct StorageConsentRequestV1 {
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) cold_root: String,
    pub(super) file_provider_domain_id: String,
    pub(super) now_ms: i64,
    pub(super) terminal_deadline_ms: i64,
}

pub(super) fn validate_storage_consent<'a>(
    state: &'a AuthorityStateModelV1,
    consent_id: &str,
    request: &StorageConsentRequestV1,
) -> Result<&'a StorageConsentV1, BoxError> {
    state.validate()?;
    let consent = state
        .storage_consents
        .get(consent_id)
        .ok_or("schedule authority: storage consent does not exist")?;
    validate_sealed_storage_consent(consent)?;
    if request.now_ms < consent.not_before_ms
        || request.terminal_deadline_ms < request.now_ms
        || request.terminal_deadline_ms > consent.expires_at_ms
        || request.operator != consent.operator
        || request.environment_owner != consent.environment_owner
        || request.cold_root != consent.cold_root
        || request.file_provider_domain_id != consent.file_provider_domain_id
        || !consent.evidence_classes.contains(&request.evidence_class)
        || state.storage_revocations.get(consent_id).copied() != Some(consent.revocation_generation)
    {
        return Err("schedule authority: storage consent binding mismatch or revoked".into());
    }
    Ok(consent)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorityStateSnapshotV1 {
    pub(super) schema_version: u16,
    pub(super) generation: u64,
    pub(super) previous_record: OptionalSha256V1,
    pub(super) recorded_at_ms: i64,
    pub(super) state: AuthorityStateModelV1,
}

impl AuthorityStateSnapshotV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.generation == 0 || self.recorded_at_ms <= 0 {
            return Err(
                "schedule authority: state snapshot must be version 1 with positive generation/time"
                    .into(),
            );
        }
        match (&self.previous_record, self.generation) {
            (OptionalSha256V1::Absent, 1) => {}
            (OptionalSha256V1::Sha256 { value }, generation) if generation > 1 => {
                require_sha256("authority previous record", value)?;
            }
            _ => {
                return Err(
                    "schedule authority: state snapshot previous-record shape is invalid".into(),
                )
            }
        }
        self.state.validate()?;
        for authorization in self.state.authorizations.values() {
            if authorization.issued_at_ms > self.recorded_at_ms {
                return Err(
                    "schedule authority: authorization was recorded before issuance".into(),
                );
            }
        }
        for runtime in self.state.one_shots.values() {
            let lifecycle_time = match &runtime.phase {
                OneShotLifecyclePhaseV1::Available => None,
                OneShotLifecyclePhaseV1::ConsumedUnreconciled { consumed_at_ms, .. }
                | OneShotLifecyclePhaseV1::Reconciled { consumed_at_ms, .. } => {
                    Some(*consumed_at_ms)
                }
            };
            if lifecycle_time.is_some_and(|time| time > self.recorded_at_ms) {
                return Err("schedule authority: lifecycle event postdates its snapshot".into());
            }
            if let OneShotLifecyclePhaseV1::Reconciled {
                reconciled_at_ms, ..
            } = &runtime.phase
            {
                if *reconciled_at_ms > self.recorded_at_ms {
                    return Err("schedule authority: reconciliation postdates its snapshot".into());
                }
            }
        }
        if self
            .state
            .manual_admissions
            .values()
            .any(|value| value.consumed_at_ms > self.recorded_at_ms)
        {
            return Err("schedule authority: manual consumption postdates its snapshot".into());
        }
        Ok(())
    }

    fn validate_repairable_projection(&self) -> Result<AuthorityStateModelV1, BoxError> {
        if self.schema_version != 1 || self.generation == 0 || self.recorded_at_ms <= 0 {
            return Err("schedule authority: state snapshot header is invalid".into());
        }
        let mut repaired = self.state.clone();
        repaired.rebuild_profile_index()?;
        repaired.validate()?;
        Ok(repaired)
    }
}

fn phase_transition_allowed(
    previous: &OneShotLifecyclePhaseV1,
    next: &OneShotLifecyclePhaseV1,
) -> bool {
    match (previous, next) {
        (OneShotLifecyclePhaseV1::Available, OneShotLifecyclePhaseV1::Available) => true,
        (
            OneShotLifecyclePhaseV1::Available,
            OneShotLifecyclePhaseV1::ConsumedUnreconciled { .. },
        ) => true,
        (
            OneShotLifecyclePhaseV1::ConsumedUnreconciled {
                admission_commit_sha256: previous_commit,
                consumed_at_ms: previous_time,
            },
            OneShotLifecyclePhaseV1::ConsumedUnreconciled {
                admission_commit_sha256: next_commit,
                consumed_at_ms: next_time,
            },
        ) => previous_commit == next_commit && previous_time == next_time,
        (
            OneShotLifecyclePhaseV1::ConsumedUnreconciled {
                admission_commit_sha256: previous_commit,
                consumed_at_ms: previous_time,
            },
            OneShotLifecyclePhaseV1::Reconciled {
                admission_commit_sha256: next_commit,
                consumed_at_ms: next_time,
                ..
            },
        ) => previous_commit == next_commit && previous_time == next_time,
        (
            OneShotLifecyclePhaseV1::Reconciled { .. },
            OneShotLifecyclePhaseV1::Reconciled { .. },
        ) => previous == next,
        _ => false,
    }
}

fn validate_authority_state_transition(
    previous: &AuthorityStateSnapshotV1,
    next: &AuthorityStateSnapshotV1,
) -> Result<(), BoxError> {
    previous.validate()?;
    next.validate()?;
    if next.generation != previous.generation + 1 || next.recorded_at_ms <= previous.recorded_at_ms
    {
        return Err("schedule authority: state generations/times are not contiguous".into());
    }
    for (id, record) in &previous.state.authorizations {
        if next.state.authorizations.get(id) != Some(record) {
            return Err("schedule authority: immutable authorization history changed".into());
        }
    }
    for (id, record) in &previous.state.grants {
        if next.state.grants.get(id) != Some(record) {
            return Err("schedule authority: immutable grant history changed".into());
        }
    }
    for (id, record) in &previous.state.storage_consents {
        if next.state.storage_consents.get(id) != Some(record) {
            return Err("schedule authority: immutable storage-consent history changed".into());
        }
    }
    for (nonce, record) in &previous.state.manual_admissions {
        if next.state.manual_admissions.get(nonce) != Some(record) {
            return Err("schedule authority: immutable manual-admission history changed".into());
        }
    }
    if next
        .state
        .manual_admissions
        .iter()
        .filter(|(nonce, _)| !previous.state.manual_admissions.contains_key(*nonce))
        .any(|(_, value)| value.consumed_at_ms < previous.recorded_at_ms)
    {
        return Err(
            "schedule authority: manual consumption predates its journal transition".into(),
        );
    }
    for (id, prior) in &previous.state.one_shots {
        let current = next
            .state
            .one_shots
            .get(id)
            .ok_or("schedule authority: one-shot lifecycle history was deleted")?;
        if prior.authorization_id != current.authorization_id
            || prior.authorization_sha256 != current.authorization_sha256
            || prior.entry_id != current.entry_id
            || prior.entry_sha256 != current.entry_sha256
            || prior.characterization_profile_sha256 != current.characterization_profile_sha256
            || current.revocation_generation < prior.revocation_generation
            || current.revocation_generation > prior.revocation_generation.saturating_add(1)
            || !phase_transition_allowed(&prior.phase, &current.phase)
        {
            return Err("schedule authority: one-shot lifecycle changed nonmonotonically".into());
        }
        match (&prior.phase, &current.phase) {
            (
                OneShotLifecyclePhaseV1::Available,
                OneShotLifecyclePhaseV1::ConsumedUnreconciled { consumed_at_ms, .. },
            ) if *consumed_at_ms < previous.recorded_at_ms => {
                return Err(
                    "schedule authority: consumption predates its journal transition".into(),
                )
            }
            (
                OneShotLifecyclePhaseV1::ConsumedUnreconciled { .. },
                OneShotLifecyclePhaseV1::Reconciled {
                    reconciled_at_ms, ..
                },
            ) if *reconciled_at_ms < previous.recorded_at_ms => {
                return Err(
                    "schedule authority: reconciliation predates its journal transition".into(),
                )
            }
            _ => {}
        }
    }
    for (id, generation) in &previous.state.grant_revocations {
        let current = next
            .state
            .grant_revocations
            .get(id)
            .ok_or("schedule authority: grant revocation history was deleted")?;
        if current < generation || *current > generation.saturating_add(1) {
            return Err("schedule authority: grant revocation changed nonmonotonically".into());
        }
    }
    for (id, generation) in &previous.state.storage_revocations {
        let current = next
            .state
            .storage_revocations
            .get(id)
            .ok_or("schedule authority: storage revocation history was deleted")?;
        if current < generation || *current > generation.saturating_add(1) {
            return Err("schedule authority: storage revocation changed nonmonotonically".into());
        }
    }
    Ok(())
}

pub(super) struct FileAuthorityJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    next_generation: u64,
    previous_sha256: Option<String>,
    previous_snapshot: Option<AuthorityStateSnapshotV1>,
}

pub(super) struct AuthorityJournalOpen<'lock> {
    pub(super) journal: FileAuthorityJournal<'lock>,
    pub(super) snapshot: AuthorityStateSnapshotV1,
    pub(super) snapshot_sha256: String,
    pub(super) projection_repair_required: bool,
}

impl<'lock> FileAuthorityJournal<'lock> {
    const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
    const MAX_GENERATIONS: usize = 10_000;
    const PREFIX: &'static str = "authority-state.";

    fn generation_name(generation: u64) -> String {
        format!("{}{generation:020}.json", Self::PREFIX)
    }

    fn generation_entries(
        directory: &local_file::PinnedDirectory,
    ) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule authority: retained state directory path changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(Self::PREFIX) {
                continue;
            }
            let Some(raw_generation) = name
                .strip_prefix(Self::PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
            else {
                return Err("schedule authority: malformed state generation name".into());
            };
            if raw_generation.len() != 20
                || !raw_generation.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err("schedule authority: malformed state generation number".into());
            }
            entries.push((raw_generation.parse::<u64>()?, name.into()));
        }
        if entries.len() > Self::MAX_GENERATIONS || !directory.current_path_matches() {
            return Err(
                "schedule authority: state generation scan is unbounded or unstable".into(),
            );
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    fn read_generation(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<(AuthorityStateSnapshotV1, Vec<u8>, String), BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let file = directory.open_regular_file(OsStr::new(name), "authority state generation")?;
        let metadata = file.metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > Self::MAX_RECORD_BYTES
        {
            return Err(
                "schedule authority: state generation is not a bounded owner-only mode-0600 file"
                    .into(),
            );
        }
        let snapshot = local_file::read_open_regular_file_bounded(
            &file,
            "authority state generation",
            Self::MAX_RECORD_BYTES,
        )?;
        let value: AuthorityStateSnapshotV1 = serde_json::from_slice(&snapshot.bytes)
            .map_err(|error| format!("schedule authority: invalid state generation: {error}"))?;
        let mut canonical = serde_json::to_vec(&value)?;
        canonical.push(b'\n');
        if canonical != snapshot.bytes {
            return Err("schedule authority: state generation is not canonical JSON".into());
        }
        Ok((value, snapshot.bytes, snapshot.sha256))
    }

    pub(super) fn initialize<C: AuthorityStateCapability + ?Sized>(
        capability: &'lock C,
        recorded_at_ms: i64,
    ) -> Result<AuthorityJournalOpen<'lock>, BoxError> {
        let directory = capability.authority_directory();
        if !Self::generation_entries(directory)?.is_empty() {
            return Err("schedule authority: state journal already exists".into());
        }
        let mut journal = Self {
            directory,
            next_generation: 1,
            previous_sha256: None,
            previous_snapshot: None,
        };
        let (snapshot, snapshot_sha256) =
            journal.append(&AuthorityStateModelV1::new(), recorded_at_ms)?;
        Ok(AuthorityJournalOpen {
            journal,
            snapshot,
            snapshot_sha256,
            projection_repair_required: false,
        })
    }

    pub(super) fn open_existing<C: AuthorityStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<AuthorityJournalOpen<'lock>, BoxError> {
        Self::open_existing_inner(capability, false)
    }

    pub(super) fn open_for_projection_repair<C: AuthorityStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<AuthorityJournalOpen<'lock>, BoxError> {
        Self::open_existing_inner(capability, true)
    }

    fn open_existing_inner<C: AuthorityStateCapability + ?Sized>(
        capability: &'lock C,
        allow_projection_repair: bool,
    ) -> Result<AuthorityJournalOpen<'lock>, BoxError> {
        let directory = capability.authority_directory();
        let entries = Self::generation_entries(directory)?;
        if entries.is_empty() {
            return Err("schedule authority: state journal has no generations".into());
        }
        let mut previous_sha256: Option<String> = None;
        let mut previous_snapshot: Option<AuthorityStateSnapshotV1> = None;
        let mut latest = None;
        let mut projection_repair_generation = None;
        for (index, (generation, name)) in entries.into_iter().enumerate() {
            let expected_generation = u64::try_from(index + 1)?;
            if generation != expected_generation {
                return Err("schedule authority: state generations are not contiguous".into());
            }
            let (mut snapshot, _bytes, sha256) = Self::read_generation(directory, &name)?;
            if snapshot.generation != generation {
                return Err("schedule authority: filename/record generation mismatch".into());
            }
            match (&snapshot.previous_record, &previous_sha256) {
                (OptionalSha256V1::Absent, None) => {}
                (OptionalSha256V1::Sha256 { value }, Some(expected)) if value == expected => {}
                _ => return Err("schedule authority: state hash chain is invalid".into()),
            }
            if let Err(error) = snapshot.validate() {
                let repaired = snapshot.validate_repairable_projection()?;
                if repaired == snapshot.state {
                    return Err(error);
                }
                snapshot.state = repaired;
                projection_repair_generation = Some(generation);
            } else if projection_repair_generation.is_some() {
                // A later valid snapshot durably supersedes an earlier non-authoritative projection.
                projection_repair_generation = None;
            }
            if let Some(previous) = &previous_snapshot {
                validate_authority_state_transition(previous, &snapshot)?;
            }
            previous_sha256 = Some(sha256.clone());
            previous_snapshot = Some(snapshot.clone());
            latest = Some((snapshot, sha256));
        }
        let (snapshot, snapshot_sha256) =
            latest.ok_or("schedule authority: state journal has no readable generation")?;
        let projection_repair_required = projection_repair_generation.is_some();
        if projection_repair_required && !allow_projection_repair {
            return Err("schedule authority: latest profile index requires repair".into());
        }
        Ok(AuthorityJournalOpen {
            journal: Self {
                directory,
                next_generation: snapshot.generation + 1,
                previous_sha256: Some(snapshot_sha256.clone()),
                previous_snapshot: Some(snapshot.clone()),
            },
            snapshot,
            snapshot_sha256,
            projection_repair_required,
        })
    }

    pub(super) fn append(
        &mut self,
        state: &AuthorityStateModelV1,
        recorded_at_ms: i64,
    ) -> Result<(AuthorityStateSnapshotV1, String), BoxError> {
        state.validate()?;
        let snapshot = AuthorityStateSnapshotV1 {
            schema_version: 1,
            generation: self.next_generation,
            previous_record: match &self.previous_sha256 {
                Some(value) => OptionalSha256V1::Sha256 {
                    value: value.clone(),
                },
                None => OptionalSha256V1::Absent,
            },
            recorded_at_ms,
            state: state.clone(),
        };
        snapshot.validate()?;
        if let Some(previous) = &self.previous_snapshot {
            validate_authority_state_transition(previous, &snapshot)?;
        }
        let mut bytes = serde_json::to_vec(&snapshot)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > Self::MAX_RECORD_BYTES {
            return Err("schedule authority: state generation exceeds the byte bound".into());
        }
        let name = Self::generation_name(self.next_generation);
        let mut file = self.directory.create_new_file(
            OsStr::new(&name),
            0o600,
            "authority state generation",
        )?;
        let write_result = file.write_all(&bytes).and_then(|_| file.sync_all());
        if let Err(error) = write_result {
            drop(file);
            let _ = self.directory.remove_child(
                OsStr::new(&name),
                false,
                "failed authority state generation",
            );
            return Err(
                format!("schedule authority: cannot persist state generation: {error}").into(),
            );
        }
        self.directory.sync()?;
        let sha256 = local_file::sha256_hex(&bytes);
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or("schedule authority: state generation overflow")?;
        self.previous_sha256 = Some(sha256.clone());
        self.previous_snapshot = Some(snapshot.clone());
        Ok((snapshot, sha256))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_schedule::{ReplicationModeV1, TriggerKindV1};
    use crate::compatibility_schedule_schema::{
        seal_admission_attempt_fingerprint, seal_case_execution_fingerprint,
        AdmissionAttemptFingerprintInputV1, AdmissionAuthorityV1, AdmissionTriggerIdentityV1,
        AggregateBudgetCapsV1, CandidateBinaryIdentityV1, CaseExecutionFingerprintInputV1,
        CharacterizationRecordV1, CharacterizedGrantProfileV1, EffectiveIdentityV1,
        ExactExecutionBindingsV1, ExactExecutionTargetV1, FingerprintV1, GitObjectAlgorithmV1,
        GitObjectIdV1, GrantBudgetPolicyV1, NamedBudgetCapsV1, OptionalGitObjectIdV1,
        OptionalRecordRefV1, OptionalSha256V1, OptionalStableIdV1, OptionalTextV1,
        ProfileSourceKindV1, TriggerBudgetCapsV1, TriggerSourceV1,
    };
    use crate::compatibility_schedule_state::SchedulerStateRoot;
    use std::os::unix::fs::PermissionsExt as _;

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    fn fingerprint(ch: char) -> FingerprintV1 {
        FingerprintV1 {
            schema_version: 1,
            sha256: digest(ch),
        }
    }

    fn text(value: &str) -> OptionalTextV1 {
        OptionalTextV1::Text {
            value: value.into(),
        }
    }

    fn identity() -> EffectiveIdentityV1 {
        EffectiveIdentityV1 {
            model: "gpt-5.6-luna".into(),
            effort: text("low"),
            mode: text("read-only"),
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

    fn foundation_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility")
    }

    fn execution_for(
        binding: &FoundationProfileBindingV1,
        caps: EffectCapsV1,
    ) -> CaseExecutionFingerprintRecordV1 {
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
            actual_caps: caps,
        })
        .unwrap()
    }

    fn one_shot_authority_for_source() -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::CharacterizationOnce(CharacterizationOnceAuthorityV1 {
            batch_authorization_id: "authorization-source-1".into(),
            batch_authorization_sha256: digest('3'),
            entry_id: "entry-source-1".into(),
            generation: 1,
            entry_sha256: digest('4'),
            consumption_nonce: "nonce-source-1".into(),
        })
    }

    fn admission_for(
        execution: &CaseExecutionFingerprintRecordV1,
        authority: AdmissionAuthorityV1,
    ) -> AdmissionAttemptFingerprintRecordV1 {
        seal_admission_attempt_fingerprint(AdmissionAttemptFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: execution.input.characterization_profile.clone(),
            case_execution: execution.fingerprint.clone(),
            authority,
            trigger: AdmissionTriggerIdentityV1 {
                source: TriggerSourceV1::ManualCharacterizationCli,
                kind: TriggerKindV1::ManualCharacterization,
                request_id: "request-source-1".into(),
                window_id: "window-source-1".into(),
                attempt_id: "attempt-source-1".into(),
                repeat_nonce: OptionalStableIdV1::Absent,
            },
        })
        .unwrap()
    }

    struct FixedNonceSource([u8; 32]);

    impl ManualNonceSource for FixedNonceSource {
        fn fill(&self, output: &mut [u8]) -> Result<(), BoxError> {
            output.copy_from_slice(&self.0);
            Ok(())
        }
    }

    fn manual_bindings() -> ManualAdmissionBindingsV1 {
        ManualAdmissionBindingsV1 {
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            scheduler_binary_sha256: digest('3'),
            input_source_sha256: digest('4'),
            characterization_profile: fingerprint('5'),
            case_execution: fingerprint('6'),
            evidence_purpose: EvidencePurposeV1::ManualDiagnostic,
            freshness_bucket: "manual-window-1".into(),
            caps: caps(),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            issued_at_ms: 10,
            expires_at_ms: 100,
        }
    }

    fn source() -> ProfileSourceRefV1 {
        ProfileSourceRefV1 {
            kind: ProfileSourceKindV1::ScheduledAdvisory,
            schema_version: 1,
            source_sha256: digest('a'),
            row_id: "case-1".into(),
            row_sha256: digest('b'),
        }
    }

    fn entry(id: &str, nonce: &str, profile: char) -> OneShotCharacterizationEntryV1 {
        OneShotCharacterizationEntryV1 {
            entry_id: id.into(),
            generation: 1,
            entry_sha256: digest('0'),
            consumption_nonce: nonce.into(),
            source: source(),
            characterization_profile: fingerprint(profile),
            characterization_execution: fingerprint('e'),
            proposed_effective_identity: identity(),
            provider_family: "openai-codex".into(),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            caps: caps(),
            command: "compatibility characterize".into(),
            not_before_ms: 10,
            expires_at_ms: 100,
            revocation_generation: 1,
            prior_entry: OptionalRecordRefV1::Absent,
            reissue_reason: OptionalTextV1::Absent,
        }
    }

    fn authorization_with(
        id: &str,
        entries: Vec<OneShotCharacterizationEntryV1>,
    ) -> CharacterizationAuthorizationV1 {
        seal_characterization_authorization(CharacterizationAuthorizationV1 {
            schema_version: 1,
            authorization_id: id.into(),
            authorization_sha256: digest('0'),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            host_identity_sha256: digest('1'),
            profile_policy_bundle_sha256: digest('2'),
            scheduler_binary_sha256: digest('3'),
            price_snapshot_sha256: digest('4'),
            legacy_inventory_sha256: digest('5'),
            issued_at_ms: 10,
            entries,
        })
        .unwrap()
    }

    fn authorization() -> CharacterizationAuthorizationV1 {
        authorization_with("authorization-1", vec![entry("entry-1", "nonce-1", 'c')])
    }

    fn environment() -> AuthorityEnvironmentV1 {
        AuthorityEnvironmentV1 {
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            host_identity_sha256: digest('1'),
            profile_policy_bundle_sha256: digest('2'),
            scheduler_binary_sha256: digest('3'),
            price_snapshot_sha256: digest('4'),
            legacy_inventory_sha256: digest('5'),
            now_ms: 20,
            terminal_deadline_ms: 50,
        }
    }

    fn characterization_request() -> CharacterizationAdmissionRequestV1 {
        CharacterizationAdmissionRequestV1 {
            entry_id: "entry-1".into(),
            source: source(),
            characterization_profile_sha256: digest('c'),
            characterization_execution_sha256: digest('e'),
            proposed_effective_identity: identity(),
            provider_family: "openai-codex".into(),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            caps: caps(),
            command: "compatibility characterize".into(),
            characterization_already_exists: false,
        }
    }

    #[test]
    fn sealed_authority_records_reject_noncanonical_or_stale_mutations() {
        let authorization = authorization();
        validate_sealed_characterization_authorization(&authorization).unwrap();
        assert_eq!(
            authorization.entries[0].entry_sha256,
            one_shot_entry_sha256(&authorization.entries[0]).unwrap()
        );

        let mut reordered = authorization.clone();
        reordered.entries[0].allowed_effects =
            vec![EffectClassV1::RegistryRead, EffectClassV1::ProviderPrompt];
        assert!(validate_sealed_characterization_authorization(&reordered).is_err());

        let mut stale_owner = authorization;
        stale_owner.operator = "Different Operator".into();
        assert!(validate_sealed_characterization_authorization(&stale_owner).is_err());

        let consent = storage_consent();
        validate_sealed_storage_consent(&consent).unwrap();
        let mut widened = consent;
        widened.evidence_classes.push(EvidenceClassV1::Incident);
        assert!(validate_sealed_storage_consent(&widened).is_err());
    }

    #[test]
    fn generated_sources_reopen_and_rederive_every_foundation_binding() {
        let root = foundation_root();
        let foundation = load_schedule_foundation(&root).unwrap();

        let scheduled_binding = &foundation.scheduled_profiles["codex-host-luna-low"];
        let scheduled_execution =
            execution_for(scheduled_binding, scheduled_binding.maximum_caps.clone());
        let scheduled_authority = one_shot_authority_for_source();
        let scheduled_admission = admission_for(&scheduled_execution, scheduled_authority.clone());
        let scheduled = generate_scheduled_execution_source(
            &root,
            "codex-host-luna-low",
            scheduled_execution,
            scheduled_admission,
            scheduled_authority,
            TriggerKindV1::ManualCharacterization,
        )
        .unwrap();
        validate_scheduled_execution_source(&root, &scheduled).unwrap();

        let mut stale_hash = scheduled.clone();
        stale_hash.config_template_sha256 = digest('9');
        assert!(validate_scheduled_execution_source(&root, &stale_hash)
            .unwrap_err()
            .to_string()
            .contains("stale hash"));

        let mut rehashed_drift = scheduled;
        rehashed_drift.config_template_sha256 = digest('9');
        let rehashed_drift = seal_scheduled_execution_source(rehashed_drift).unwrap();
        assert!(validate_scheduled_execution_source(&root, &rehashed_drift)
            .unwrap_err()
            .to_string()
            .contains("rederived checked-in foundation"));

        let claimed_binding = &foundation.claimed_support_profiles["codex-host-bridge-gpt56-sol"];
        let claimed_execution =
            execution_for(claimed_binding, claimed_binding.maximum_caps.clone());
        let claimed_authority = one_shot_authority_for_source();
        let claimed_admission = admission_for(&claimed_execution, claimed_authority.clone());
        let claimed = generate_claimed_support_characterization_source(
            &root,
            "codex-host-bridge-gpt56-sol",
            claimed_execution,
            claimed_admission,
            claimed_authority,
        )
        .unwrap();
        validate_claimed_support_characterization_source(&root, &claimed).unwrap();

        let mut wrong_exact_config = claimed;
        wrong_exact_config.pinned_config_sha256 = digest('9');
        wrong_exact_config
            .characterization_execution
            .input
            .bindings
            .generated_config_sha256 = digest('9');
        wrong_exact_config.characterization_execution = seal_case_execution_fingerprint(
            wrong_exact_config.characterization_execution.input.clone(),
        )
        .unwrap();
        wrong_exact_config.admission_attempt = admission_for(
            &wrong_exact_config.characterization_execution,
            wrong_exact_config.authority.clone(),
        );
        let wrong_exact_config =
            seal_claimed_support_characterization_source(wrong_exact_config).unwrap();
        assert!(
            validate_claimed_support_characterization_source(&root, &wrong_exact_config)
                .unwrap_err()
                .to_string()
                .contains("exact pins drifted")
        );
    }

    #[test]
    fn source_generation_refuses_unknown_rows_and_caps_above_the_profile_maximum() {
        let root = foundation_root();
        let foundation = load_schedule_foundation(&root).unwrap();
        let binding = &foundation.scheduled_profiles["codex-host-luna-low"];
        let authority = one_shot_authority_for_source();
        let execution = execution_for(binding, binding.maximum_caps.clone());
        let admission = admission_for(&execution, authority.clone());
        assert!(generate_scheduled_execution_source(
            &root,
            "unknown-row",
            execution,
            admission,
            authority,
            TriggerKindV1::ManualCharacterization,
        )
        .is_err());

        let mut widened_caps = binding.maximum_caps.clone();
        widened_caps.max_tokens += 1;
        let execution = execution_for(binding, widened_caps);
        let authority = one_shot_authority_for_source();
        let admission = admission_for(&execution, authority.clone());
        assert!(generate_scheduled_execution_source(
            &root,
            "codex-host-luna-low",
            execution,
            admission,
            authority,
            TriggerKindV1::ManualCharacterization,
        )
        .unwrap_err()
        .to_string()
        .contains("exceeds the checked-in profile maximum"));
    }

    #[test]
    fn direct_manual_derivation_mints_and_consumes_one_nonreplayable_nonce() {
        let nonce_source = FixedNonceSource([7; 32]);
        let sealed = derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &nonce_source,
            manual_bindings(),
        )
        .unwrap();
        validate_sealed_manual_admission(&sealed).unwrap();
        assert_eq!(
            sealed.record.request_nonce,
            local_file::sha256_hex(&[7; 32])
        );
        assert!(matches!(
            sealed.authority,
            AdmissionAuthorityV1::ManualAcknowledgement(_)
        ));
        let manual_nonce = sealed.record.request_nonce.clone();

        let mut state = AuthorityStateModelV1::new();
        state
            .consume_manual_admission(sealed.clone(), &digest('8'), 20)
            .unwrap();
        assert_eq!(state.manual_admissions.len(), 1);
        let before_replay = state.clone();
        assert!(state
            .consume_manual_admission(sealed, &digest('9'), 21)
            .is_err());
        assert_eq!(state, before_replay);

        let mut conflicting_entry = entry("entry-1", &manual_nonce, 'c');
        conflicting_entry.entry_sha256 = digest('0');
        assert!(state
            .issue_authorization(authorization_with(
                "authorization-1",
                vec![conflicting_entry],
            ))
            .is_err());
        assert_eq!(state, before_replay);

        let mut random = [0_u8; 32];
        SystemManualNonceSource.fill(&mut random).unwrap();
    }

    #[test]
    fn manual_derivation_refuses_nonlocal_origins_ack_and_caller_nonce() {
        let nonce_source = FixedNonceSource([7; 32]);
        for origin in [
            ManualAdmissionOriginV1::Serve,
            ManualAdmissionOriginV1::A2a,
            ManualAdmissionOriginV1::Timer,
            ManualAdmissionOriginV1::Watcher,
        ] {
            assert!(
                derive_manual_admission(origin, true, None, &nonce_source, manual_bindings(),)
                    .is_err()
            );
        }
        assert!(derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            false,
            None,
            &nonce_source,
            manual_bindings(),
        )
        .is_err());
        assert!(derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            Some("caller-chosen"),
            &nonce_source,
            manual_bindings(),
        )
        .is_err());
    }

    #[test]
    fn manual_seal_refuses_mutation_characterization_and_overlong_expiry() {
        let nonce_source = FixedNonceSource([7; 32]);
        let sealed = derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &nonce_source,
            manual_bindings(),
        )
        .unwrap();

        let mut mutated = sealed.clone();
        mutated.record.input_source_sha256 = digest('9');
        assert!(validate_sealed_manual_admission(&mutated).is_err());

        let mut characterization = manual_bindings();
        characterization.evidence_purpose = EvidencePurposeV1::Characterization;
        assert!(derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &nonce_source,
            characterization,
        )
        .is_err());

        let mut overlong = manual_bindings();
        overlong.expires_at_ms = overlong.issued_at_ms + MAX_MANUAL_ADMISSION_LIFETIME_MS + 1;
        assert!(derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &nonce_source,
            overlong,
        )
        .unwrap_err()
        .to_string()
        .contains("one-run lifetime"));
    }

    #[test]
    fn one_shot_lifecycle_is_unique_nonreplayable_and_requires_linear_reissue() {
        let mut state = AuthorityStateModelV1::new();
        state.issue_authorization(authorization()).unwrap();

        let duplicate =
            authorization_with("authorization-2", vec![entry("entry-2", "nonce-2", 'c')]);
        assert!(state.issue_authorization(duplicate).is_err());

        state.consume_one_shot("entry-1", &digest('6'), 30).unwrap();
        assert!(state.consume_one_shot("entry-1", &digest('6'), 31).is_err());
        state.rollback_provider_authority().unwrap();
        assert!(state
            .issue_authorization(authorization_with(
                "authorization-3",
                vec![entry("entry-3", "nonce-3", 'c')],
            ))
            .is_err());

        state
            .reconcile_one_shot("entry-1", &digest('6'), &digest('7'), 40)
            .unwrap();
        let prior = state.static_entry("entry-1").unwrap().clone();
        let mut reissue = entry("entry-2", "nonce-2", 'c');
        reissue.generation = 2;
        reissue.prior_entry = OptionalRecordRefV1::Record {
            id: prior.entry_id,
            sha256: prior.entry_sha256,
        };
        reissue.reissue_reason = text("operator reviewed the reconciled prior outcome");
        state
            .issue_authorization(authorization_with("authorization-2", vec![reissue]))
            .unwrap();

        let mut corrupt = state.clone();
        corrupt.profile_index.clear();
        assert!(corrupt.validate().is_err());
        corrupt.rebuild_profile_index().unwrap();
        corrupt.validate().unwrap();
    }

    #[test]
    fn rejected_authority_mutation_leaves_the_previous_state_intact() {
        let mut state = AuthorityStateModelV1::new();
        let duplicate_nonce = authorization_with(
            "authorization-1",
            vec![
                entry("entry-1", "same-nonce", 'c'),
                entry("entry-2", "same-nonce", 'd'),
            ],
        );

        assert!(state.issue_authorization(duplicate_nonce).is_err());
        state.validate().unwrap();
        assert!(state.authorizations.is_empty());
        assert!(state.one_shots.is_empty());
        assert!(state.profile_index.is_empty());
    }

    #[test]
    fn one_shot_selection_fences_all_bound_identity_and_revocation_state() {
        let mut state = AuthorityStateModelV1::new();
        state.issue_authorization(authorization()).unwrap();
        let selected = select_characterization_authority(
            &state,
            "authorization-1",
            &environment(),
            &characterization_request(),
        )
        .unwrap();
        assert!(matches!(
            selected,
            AdmissionAuthorityV1::CharacterizationOnce(_)
        ));

        let mut wrong_owner = environment();
        wrong_owner.environment_owner = "different-owner".into();
        let mut wrong_operator = environment();
        wrong_operator.operator = "Different Operator".into();
        let mut wrong_host = environment();
        wrong_host.host_identity_sha256 = digest('9');
        let mut wrong_bundle = environment();
        wrong_bundle.profile_policy_bundle_sha256 = digest('9');
        let mut wrong_binary = environment();
        wrong_binary.scheduler_binary_sha256 = digest('9');
        let mut wrong_price = environment();
        wrong_price.price_snapshot_sha256 = digest('9');
        let mut wrong_legacy = environment();
        wrong_legacy.legacy_inventory_sha256 = digest('9');
        for mismatch in [
            wrong_owner,
            wrong_operator,
            wrong_host,
            wrong_bundle,
            wrong_binary,
            wrong_price,
            wrong_legacy,
        ] {
            assert!(select_characterization_authority(
                &state,
                "authorization-1",
                &mismatch,
                &characterization_request(),
            )
            .is_err());
        }

        let mut expired = environment();
        expired.terminal_deadline_ms = 101;
        assert!(select_characterization_authority(
            &state,
            "authorization-1",
            &expired,
            &characterization_request(),
        )
        .is_err());

        let mut widened = characterization_request();
        widened.allowed_effects.push(EffectClassV1::RegistryRead);
        assert!(select_characterization_authority(
            &state,
            "authorization-1",
            &environment(),
            &widened,
        )
        .is_err());

        let mut wrong_source = characterization_request();
        wrong_source.source.row_id = "different-case".into();
        let mut wrong_profile = characterization_request();
        wrong_profile.characterization_profile_sha256 = digest('9');
        let mut wrong_execution = characterization_request();
        wrong_execution.characterization_execution_sha256 = digest('9');
        let mut wrong_identity = characterization_request();
        wrong_identity.proposed_effective_identity.model = "different-model".into();
        let mut wrong_provider = characterization_request();
        wrong_provider.provider_family = "different-provider".into();
        let mut wrong_caps = characterization_request();
        wrong_caps.caps.max_tokens += 1;
        let mut wrong_command = characterization_request();
        wrong_command.command = "compatibility run".into();
        for mismatch in [
            wrong_source,
            wrong_profile,
            wrong_execution,
            wrong_identity,
            wrong_provider,
            wrong_caps,
            wrong_command,
        ] {
            assert!(select_characterization_authority(
                &state,
                "authorization-1",
                &environment(),
                &mismatch,
            )
            .is_err());
        }

        let mut already_characterized = characterization_request();
        already_characterized.characterization_already_exists = true;
        assert!(select_characterization_authority(
            &state,
            "authorization-1",
            &environment(),
            &already_characterized,
        )
        .is_err());

        state.rollback_provider_authority().unwrap();
        assert!(select_characterization_authority(
            &state,
            "authorization-1",
            &environment(),
            &characterization_request(),
        )
        .is_err());
        assert!(matches!(
            selected,
            AdmissionAuthorityV1::CharacterizationOnce(_)
        ));
    }

    fn aggregate_caps(attempts: u64, tokens: u64) -> AggregateBudgetCapsV1 {
        AggregateBudgetCapsV1 {
            max_attempts: attempts,
            max_tokens: tokens,
            max_cost_microusd: attempts * 10_000,
            max_time_secs: attempts * 1_000,
        }
    }

    fn characterization(authority: AdmissionAuthorityV1) -> CharacterizationRecordV1 {
        CharacterizationRecordV1 {
            schema_version: 1,
            characterization_id: "characterization-1".into(),
            source: source(),
            profile_policy_bundle_sha256: digest('2'),
            characterization_profile: fingerprint('c'),
            case_execution: fingerprint('e'),
            admission_attempt: fingerprint('f'),
            authority,
            expected_effective_identity: identity(),
            observed_effective_identity: identity(),
            outcome: CharacterizationOutcomeV1::CharacterizedGreen,
            evidence_sha256: digest('8'),
            terminal_at_ms: 19,
        }
    }

    fn grant(characterization: &CharacterizationRecordV1) -> ProviderEffectGrantV1 {
        let characterization_sha256 = characterization_record_sha256(characterization).unwrap();
        let pool = aggregate_caps(1, 100);
        seal_provider_effect_grant(ProviderEffectGrantV1 {
            schema_version: 1,
            grant_id: "grant-1".into(),
            generation: 1,
            grant_sha256: digest('0'),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            host_identity_sha256: digest('1'),
            profile_policy_bundle_sha256: digest('2'),
            scheduler_binary_sha256: digest('3'),
            price_snapshot_sha256: digest('4'),
            price_snapshot_observed_at_ms: 10,
            price_snapshot_valid_until_ms: 100,
            legacy_inventory_sha256: digest('5'),
            triggers: vec![TriggerKindV1::Daily],
            case_ids: vec!["case-1".into()],
            provider_families: vec!["openai-codex".into()],
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            per_run_caps: caps(),
            budgets: GrantBudgetPolicyV1 {
                per_case: vec![NamedBudgetCapsV1 {
                    id: "case-1".into(),
                    caps: pool.clone(),
                }],
                per_trigger_pool: vec![TriggerBudgetCapsV1 {
                    trigger: TriggerKindV1::Daily,
                    caps: pool.clone(),
                }],
                per_provider: vec![NamedBudgetCapsV1 {
                    id: "openai-codex".into(),
                    caps: pool.clone(),
                }],
                utc_day: aggregate_caps(3, 300),
                rolling_24h: aggregate_caps(3, 300),
                protected_scheduled: pool.clone(),
                protected_test_merge: pool.clone(),
                manual_unallocated: pool,
            },
            confirmation_allowance: 1,
            launchd: vec![LaunchdBindingV1 {
                label: "com.a2a-bridge.compatibility.daily".into(),
                plist_sha256: digest('9'),
                trigger: TriggerKindV1::Daily,
            }],
            profiles: vec![CharacterizedGrantProfileV1 {
                case_id: "case-1".into(),
                provider_family: "openai-codex".into(),
                source: source(),
                characterization_profile: fingerprint('c'),
                characterization_id: characterization.characterization_id.clone(),
                characterization_sha256,
                effective_identity: identity(),
                caps: caps(),
            }],
            not_before_ms: 10,
            expires_at_ms: 100,
            revocation_generation: 1,
        })
        .unwrap()
    }

    fn standing_request(characterization: CharacterizationRecordV1) -> StandingAdmissionRequestV1 {
        StandingAdmissionRequestV1 {
            trigger: TriggerKindV1::Daily,
            case_id: "case-1".into(),
            provider_family: "openai-codex".into(),
            source: source(),
            characterization_profile_sha256: digest('c'),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            caps: caps(),
            launchd: Some(LaunchdBindingV1 {
                label: "com.a2a-bridge.compatibility.daily".into(),
                plist_sha256: digest('9'),
                trigger: TriggerKindV1::Daily,
            }),
            characterization,
        }
    }

    #[test]
    fn standing_grant_requires_exact_completed_characterization_and_launchd_binding() {
        let mut state = AuthorityStateModelV1::new();
        state.issue_authorization(authorization()).unwrap();
        let one_shot = select_characterization_authority(
            &state,
            "authorization-1",
            &environment(),
            &characterization_request(),
        )
        .unwrap();
        let characterization = characterization(one_shot);
        state.install_grant(grant(&characterization)).unwrap();

        let selected = select_standing_grant(
            &state,
            "grant-1",
            &environment(),
            &standing_request(characterization.clone()),
        )
        .unwrap();
        assert!(matches!(selected, AdmissionAuthorityV1::StandingGrant(_)));

        let mut second_grant = grant(&characterization);
        second_grant.grant_id = "grant-2".into();
        let second_grant = seal_provider_effect_grant(second_grant).unwrap();
        assert!(state.install_grant(second_grant).is_err());
        state.validate().unwrap();
        assert_eq!(state.grants.len(), 1);

        let mut inconclusive = characterization.clone();
        inconclusive.outcome = CharacterizationOutcomeV1::CharacterizationInconclusive;
        assert!(select_standing_grant(
            &state,
            "grant-1",
            &environment(),
            &standing_request(inconclusive),
        )
        .is_err());

        let mut wrong_launchd = standing_request(characterization.clone());
        wrong_launchd.launchd.as_mut().unwrap().plist_sha256 = digest('0');
        assert!(select_standing_grant(&state, "grant-1", &environment(), &wrong_launchd,).is_err());

        state.rollback_provider_authority().unwrap();
        assert!(select_standing_grant(
            &state,
            "grant-1",
            &environment(),
            &standing_request(characterization),
        )
        .is_err());
        assert!(matches!(selected, AdmissionAuthorityV1::StandingGrant(_)));
    }

    fn storage_consent() -> StorageConsentV1 {
        seal_storage_consent(StorageConsentV1 {
            schema_version: 1,
            consent_id: "consent-1".into(),
            consent_sha256: digest('0'),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            evidence_classes: vec![EvidenceClassV1::RoutineGreen],
            cold_root: "~/Documents/a2a-bridge/evidence-archive".into(),
            replication_mode: ReplicationModeV1::OwnerIcloud,
            file_provider_domain_id: "icloud-domain-1".into(),
            not_before_ms: 10,
            expires_at_ms: 100,
            revocation_generation: 1,
        })
        .unwrap()
    }

    fn storage_request() -> StorageConsentRequestV1 {
        StorageConsentRequestV1 {
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            evidence_class: EvidenceClassV1::RoutineGreen,
            cold_root: "~/Documents/a2a-bridge/evidence-archive".into(),
            file_provider_domain_id: "icloud-domain-1".into(),
            now_ms: 20,
            terminal_deadline_ms: 50,
        }
    }

    #[test]
    fn storage_consent_is_independent_from_provider_authority() {
        let mut state = AuthorityStateModelV1::new();
        state.issue_authorization(authorization()).unwrap();
        state.install_storage_consent(storage_consent()).unwrap();
        validate_storage_consent(&state, "consent-1", &storage_request()).unwrap();

        state.rollback_provider_authority().unwrap();
        validate_storage_consent(&state, "consent-1", &storage_request()).unwrap();
        assert!(select_characterization_authority(
            &state,
            "authorization-1",
            &environment(),
            &characterization_request(),
        )
        .is_err());

        state.revoke_storage_consent("consent-1").unwrap();
        assert!(validate_storage_consent(&state, "consent-1", &storage_request()).is_err());

        let mut provider_state = AuthorityStateModelV1::new();
        provider_state.issue_authorization(authorization()).unwrap();
        let one_shot = select_characterization_authority(
            &provider_state,
            "authorization-1",
            &environment(),
            &characterization_request(),
        )
        .unwrap();
        let characterization = characterization(one_shot);
        provider_state
            .install_grant(grant(&characterization))
            .unwrap();
        provider_state
            .install_storage_consent(storage_consent())
            .unwrap();
        provider_state.revoke_storage_consent("consent-1").unwrap();
        assert!(
            validate_storage_consent(&provider_state, "consent-1", &storage_request(),).is_err()
        );
        assert!(select_standing_grant(
            &provider_state,
            "grant-1",
            &environment(),
            &standing_request(characterization),
        )
        .is_ok());
    }

    fn journal_root() -> (tempfile::TempDir, SchedulerStateRoot) {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state_root = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        (root, state_root)
    }

    #[test]
    fn authority_journal_reopens_a_contiguous_hash_chain_and_refuses_history_rewrite() {
        let (root, state_root) = journal_root();
        let authority_lock = state_root
            .try_authority_mutation("operator:authority-journal")
            .unwrap();
        let opened = FileAuthorityJournal::initialize(&authority_lock, 1).unwrap();
        assert_eq!(opened.snapshot.generation, 1);
        assert!(matches!(
            opened.snapshot.previous_record,
            OptionalSha256V1::Absent
        ));
        let mut state = opened.snapshot.state.clone();
        state.issue_authorization(authorization()).unwrap();
        let mut journal = opened.journal;
        let (second, second_sha256) = journal.append(&state, 20).unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(second.state, state);
        assert!(local_file::valid_sha256(&second_sha256));

        let empty = AuthorityStateModelV1::new();
        assert!(journal.append(&empty, 21).is_err());
        assert!(!root
            .path()
            .join("authority/authority-state.00000000000000000003.json")
            .exists());
        drop(journal);
        drop(authority_lock);

        let authority_lock = state_root
            .try_authority_mutation("operator:authority-reopen")
            .unwrap();
        let reopened = FileAuthorityJournal::open_existing(&authority_lock).unwrap();
        assert_eq!(reopened.snapshot.generation, 2);
        assert_eq!(reopened.snapshot.state, state);
        assert_eq!(reopened.snapshot_sha256, second_sha256);
        assert!(!reopened.projection_repair_required);
        for generation in [1, 2] {
            let metadata = std::fs::metadata(
                root.path()
                    .join(format!("authority/authority-state.{generation:020}.json")),
            )
            .unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn manual_consumption_is_immutable_across_authority_journal_recovery() {
        let (_root, state_root) = journal_root();
        let authority_lock = state_root
            .try_authority_mutation("operator:manual-setup")
            .unwrap();
        let opened = FileAuthorityJournal::initialize(&authority_lock, 1).unwrap();
        let sealed = derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &FixedNonceSource([7; 32]),
            manual_bindings(),
        )
        .unwrap();
        let mut consumed = opened.snapshot.state.clone();
        consumed
            .consume_manual_admission(sealed, &digest('8'), 20)
            .unwrap();
        let mut journal = opened.journal;
        journal.append(&consumed, 21).unwrap();
        drop(journal);
        drop(authority_lock);

        let authority_lock = state_root
            .try_authority_mutation("operator:manual-reopen")
            .unwrap();
        let reopened = FileAuthorityJournal::open_existing(&authority_lock).unwrap();
        assert_eq!(reopened.snapshot.state.manual_admissions.len(), 1);
        let mut journal = reopened.journal;
        assert!(journal.append(&AuthorityStateModelV1::new(), 30).is_err());
    }

    #[test]
    fn corrupt_profile_projection_repairs_without_trusting_it_but_authority_corruption_holds() {
        let (root, state_root) = journal_root();
        let authority_lock = state_root
            .try_authority_mutation("operator:projection-setup")
            .unwrap();
        let opened = FileAuthorityJournal::initialize(&authority_lock, 1).unwrap();
        let mut state = opened.snapshot.state.clone();
        state.issue_authorization(authorization()).unwrap();
        let mut journal = opened.journal;
        journal.append(&state, 20).unwrap();
        drop(journal);
        drop(authority_lock);

        let second_path = root
            .path()
            .join("authority/authority-state.00000000000000000002.json");
        let mut second: AuthorityStateSnapshotV1 =
            serde_json::from_slice(&std::fs::read(&second_path).unwrap()).unwrap();
        second.state.profile_index.clear();
        let mut bytes = serde_json::to_vec(&second).unwrap();
        bytes.push(b'\n');
        std::fs::write(&second_path, bytes).unwrap();

        let authority_lock = state_root
            .try_authority_mutation("operator:projection-repair")
            .unwrap();
        assert!(FileAuthorityJournal::open_existing(&authority_lock).is_err());
        let repaired = FileAuthorityJournal::open_for_projection_repair(&authority_lock).unwrap();
        assert!(repaired.projection_repair_required);
        assert_eq!(repaired.snapshot.state, state);
        let mut journal = repaired.journal;
        journal.append(&repaired.snapshot.state, 30).unwrap();
        drop(journal);
        drop(authority_lock);

        let authority_lock = state_root
            .try_authority_mutation("operator:projection-confirm")
            .unwrap();
        let reopened = FileAuthorityJournal::open_existing(&authority_lock).unwrap();
        assert_eq!(reopened.snapshot.generation, 3);
        assert!(!reopened.projection_repair_required);
        drop(reopened);
        drop(authority_lock);

        let third_path = root
            .path()
            .join("authority/authority-state.00000000000000000003.json");
        let mut third: AuthorityStateSnapshotV1 =
            serde_json::from_slice(&std::fs::read(&third_path).unwrap()).unwrap();
        third
            .state
            .authorizations
            .get_mut("authorization-1")
            .unwrap()
            .authorization_sha256 = digest('f');
        let mut bytes = serde_json::to_vec(&third).unwrap();
        bytes.push(b'\n');
        std::fs::write(&third_path, bytes).unwrap();

        let authority_lock = state_root
            .try_authority_mutation("operator:corruption-confirm")
            .unwrap();
        assert!(FileAuthorityJournal::open_existing(&authority_lock).is_err());
        assert!(FileAuthorityJournal::open_for_projection_repair(&authority_lock).is_err());
    }

    #[test]
    fn journal_crash_boundary_keeps_precommit_available_and_postcommit_nonreplayable() {
        let (root, state_root) = journal_root();
        let authority_lock = state_root
            .try_authority_mutation("operator:crash-setup")
            .unwrap();
        let opened = FileAuthorityJournal::initialize(&authority_lock, 1).unwrap();
        let mut state = opened.snapshot.state.clone();
        state.issue_authorization(authorization()).unwrap();
        let mut journal = opened.journal;
        journal.append(&state, 20).unwrap();
        drop(journal);
        drop(authority_lock);

        let authority_lock = state_root
            .try_authority_mutation("operator:before-consumption")
            .unwrap();
        let before = FileAuthorityJournal::open_existing(&authority_lock).unwrap();
        assert!(select_characterization_authority(
            &before.snapshot.state,
            "authorization-1",
            &environment(),
            &characterization_request(),
        )
        .is_ok());
        let mut consumed = before.snapshot.state.clone();
        consumed
            .consume_one_shot("entry-1", &digest('6'), 25)
            .unwrap();
        let mut journal = before.journal;
        journal.append(&consumed, 25).unwrap();
        drop(journal);
        drop(authority_lock);

        let authority_lock = state_root
            .try_authority_mutation("operator:after-consumption")
            .unwrap();
        let after = FileAuthorityJournal::open_existing(&authority_lock).unwrap();
        assert!(matches!(
            after.snapshot.state.one_shots["entry-1"].phase,
            OneShotLifecyclePhaseV1::ConsumedUnreconciled { .. }
        ));
        assert!(select_characterization_authority(
            &after.snapshot.state,
            "authorization-1",
            &environment(),
            &characterization_request(),
        )
        .is_err());
        drop(after);
        drop(authority_lock);

        let partial = root
            .path()
            .join("authority/authority-state.00000000000000000004.json");
        std::fs::write(&partial, b"{").unwrap();
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600)).unwrap();
        let authority_lock = state_root
            .try_authority_mutation("operator:partial-confirm")
            .unwrap();
        assert!(FileAuthorityJournal::open_existing(&authority_lock).is_err());
        assert!(FileAuthorityJournal::open_for_projection_repair(&authority_lock).is_err());
    }
}
