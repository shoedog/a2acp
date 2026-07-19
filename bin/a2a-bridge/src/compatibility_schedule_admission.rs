//! Final-fence identities, equivalent-work state, and disjoint control reducers for R3d2.
//!
//! The module is provider-effect free. It recomputes sealed identities from already validated local
//! sources, reduces proposed admission state through copy-validate-commit mutations, and remains
//! unreachable from `schedule-tick` until the shared R3d2e transaction is complete.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule::{load_schedule_foundation, EvidencePurposeV1, TriggerKindV1};
use crate::compatibility_schedule_authority::{
    characterization_record_sha256, validate_claimed_support_characterization_source,
    validate_scheduled_execution_source, validate_sealed_manual_admission, SealedManualAdmissionV1,
};
use crate::compatibility_schedule_schema::{
    safety_hold_clearance_action_sha256, safety_hold_opening_sha256,
    seal_admission_attempt_fingerprint, seal_case_execution_fingerprint,
    AdmissionAttemptFingerprintInputV1, AdmissionAttemptFingerprintRecordV1, AdmissionAuthorityV1,
    AdmissionTriggerIdentityV1, CaseExecutionFingerprintInputV1, CaseExecutionFingerprintRecordV1,
    CharacterizationOutcomeV1, CharacterizationRecordV1, ClaimedSupportCharacterizationSourceV1,
    ConsumptionEvidenceProvenanceV1, ConsumptionRecordV1, EffectiveIdentityV1,
    EquivalentWorkReservationV1, FailureActionV1, FailureDispositionV1, FailureKindV1,
    FingerprintV1, HoldLifecycleV1, HoldReasonV1, OptionalStableIdV1, QuarantineV1, SafetyHoldV1,
    ScheduledExecutionSourceV1, ValidateRecord,
};
use crate::{local_file, BoxError};

fn admission_hash<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| format!("schedule admission: cannot canonicalize {label}: {error}"))?;
    let mut bytes = format!("a2a-bridge:r3d2:{label}:v1\0").into_bytes();
    bytes.extend_from_slice(&canonical);
    Ok(local_file::sha256_hex(&bytes))
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
        return Err(format!("schedule admission: {label} is not a bounded stable id").into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct EquivalentWorkKeyInputV1<'a> {
    schema_version: u16,
    case_execution: &'a FingerprintV1,
    evidence_purpose: EvidencePurposeV1,
    freshness_bucket: &'a str,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AttemptIdempotencyKeyInputV1<'a> {
    schema_version: u16,
    admission_attempt: &'a FingerprintV1,
    repeat_nonce: &'a OptionalStableIdV1,
}

pub(super) fn equivalent_work_key(
    case_execution: &FingerprintV1,
    evidence_purpose: EvidencePurposeV1,
    freshness_bucket: &str,
) -> Result<String, BoxError> {
    stable_id("freshness bucket", freshness_bucket)?;
    admission_hash(
        "equivalent-work-key",
        &EquivalentWorkKeyInputV1 {
            schema_version: 1,
            case_execution,
            evidence_purpose,
            freshness_bucket,
        },
    )
}

pub(super) fn attempt_idempotency_key(
    admission_attempt: &FingerprintV1,
    repeat_nonce: &OptionalStableIdV1,
) -> Result<String, BoxError> {
    admission_hash(
        "attempt-idempotency-key",
        &AttemptIdempotencyKeyInputV1 {
            schema_version: 1,
            admission_attempt,
            repeat_nonce,
        },
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DerivedAdmissionIdentitiesV1 {
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: CaseExecutionFingerprintRecordV1,
    pub(super) admission_attempt: AdmissionAttemptFingerprintRecordV1,
    pub(super) equivalent_work_key: String,
    pub(super) attempt_idempotency_key: String,
    pub(super) evidence_purpose: EvidencePurposeV1,
    pub(super) freshness_bucket: String,
}

fn derive_admission_identities(
    expected_profile: &FingerprintV1,
    case_execution_input: CaseExecutionFingerprintInputV1,
    authority: AdmissionAuthorityV1,
    trigger: AdmissionTriggerIdentityV1,
    evidence_purpose: EvidencePurposeV1,
    freshness_bucket: String,
) -> Result<DerivedAdmissionIdentitiesV1, BoxError> {
    if &case_execution_input.characterization_profile != expected_profile {
        return Err("schedule admission: execution does not bind the rederived profile".into());
    }
    let case_execution = seal_case_execution_fingerprint(case_execution_input)?;
    let admission_attempt =
        seal_admission_attempt_fingerprint(AdmissionAttemptFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: expected_profile.clone(),
            case_execution: case_execution.fingerprint.clone(),
            authority,
            trigger,
        })?;
    let equivalent_work_key = equivalent_work_key(
        &case_execution.fingerprint,
        evidence_purpose,
        &freshness_bucket,
    )?;
    let attempt_idempotency_key = attempt_idempotency_key(
        &admission_attempt.fingerprint,
        &admission_attempt.input.trigger.repeat_nonce,
    )?;
    Ok(DerivedAdmissionIdentitiesV1 {
        characterization_profile: expected_profile.clone(),
        case_execution,
        admission_attempt,
        equivalent_work_key,
        attempt_idempotency_key,
        evidence_purpose,
        freshness_bucket,
    })
}

pub(super) fn rederive_scheduled_identities(
    foundation_root: &Path,
    source: &ScheduledExecutionSourceV1,
    freshness_bucket: String,
) -> Result<DerivedAdmissionIdentitiesV1, BoxError> {
    validate_scheduled_execution_source(foundation_root, source)?;
    let foundation = load_schedule_foundation(foundation_root)?;
    let binding = foundation
        .scheduled_profiles
        .get(&source.source.row_id)
        .ok_or("schedule admission: scheduled profile disappeared during final rederivation")?;
    let purpose = if source.trigger == TriggerKindV1::ManualCharacterization {
        EvidencePurposeV1::Characterization
    } else {
        binding.evidence_purpose
    };
    let derived = derive_admission_identities(
        &binding.characterization_profile,
        source.case_execution.input.clone(),
        source.authority.clone(),
        source.admission_attempt.input.trigger.clone(),
        purpose,
        freshness_bucket,
    )?;
    if derived.case_execution != source.case_execution
        || derived.admission_attempt != source.admission_attempt
    {
        return Err("schedule admission: scheduled source fingerprints were not rederived".into());
    }
    Ok(derived)
}

pub(super) fn rederive_claimed_support_identities(
    foundation_root: &Path,
    source: &ClaimedSupportCharacterizationSourceV1,
    freshness_bucket: String,
) -> Result<DerivedAdmissionIdentitiesV1, BoxError> {
    validate_claimed_support_characterization_source(foundation_root, source)?;
    let derived = derive_admission_identities(
        &source.characterization_profile,
        source.characterization_execution.input.clone(),
        source.authority.clone(),
        source.admission_attempt.input.trigger.clone(),
        EvidencePurposeV1::Characterization,
        freshness_bucket,
    )?;
    if derived.case_execution != source.characterization_execution
        || derived.admission_attempt != source.admission_attempt
    {
        return Err("schedule admission: claimed-support fingerprints were not rederived".into());
    }
    Ok(derived)
}

pub(super) fn rederive_manual_identities(
    manual: &SealedManualAdmissionV1,
    case_execution_input: CaseExecutionFingerprintInputV1,
    trigger: AdmissionTriggerIdentityV1,
) -> Result<DerivedAdmissionIdentitiesV1, BoxError> {
    validate_sealed_manual_admission(manual)?;
    if case_execution_input.characterization_profile != manual.record.characterization_profile
        || case_execution_input.actual_caps != manual.record.caps
    {
        return Err(
            "schedule admission: manual execution differs from its sealed admission".into(),
        );
    }
    let derived = derive_admission_identities(
        &manual.record.characterization_profile,
        case_execution_input,
        manual.authority.clone(),
        trigger,
        manual.record.evidence_purpose,
        manual.record.freshness_bucket.clone(),
    )?;
    if derived.case_execution.fingerprint != manual.record.case_execution {
        return Err("schedule admission: manual case-execution fingerprint drifted".into());
    }
    Ok(derived)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CompletedEquivalentEvidenceV1 {
    pub(super) reservation_id: String,
    pub(super) evidence_sha256: String,
    pub(super) satisfied_purpose: EvidencePurposeV1,
    pub(super) freshness_bucket: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) expected_effective_identity: EffectiveIdentityV1,
    pub(super) observed_effective_identity: EffectiveIdentityV1,
    pub(super) provenance: ConsumptionEvidenceProvenanceV1,
    pub(super) reusable: bool,
    pub(super) terminal_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct EquivalentWorkStateV1 {
    #[serde(default)]
    pub(super) reservations: BTreeMap<String, EquivalentWorkReservationV1>,
    #[serde(default)]
    pub(super) live_by_execution: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) completed: BTreeMap<String, CompletedEquivalentEvidenceV1>,
    #[serde(default)]
    pub(super) consumptions: BTreeMap<String, ConsumptionRecordV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EquivalentWorkDecisionV1 {
    Reserved(EquivalentWorkReservationV1),
    Reused(ConsumptionRecordV1),
}

fn purpose_satisfies(
    requested: EvidencePurposeV1,
    satisfied: EvidencePurposeV1,
    provenance: &ConsumptionEvidenceProvenanceV1,
) -> bool {
    if matches!(
        requested,
        EvidencePurposeV1::Characterization | EvidencePurposeV1::ManualDiagnostic
    ) {
        return false;
    }
    match provenance {
        ConsumptionEvidenceProvenanceV1::Ordinary => {
            requested == satisfied
                || (requested == EvidencePurposeV1::ProviderPathAdvisory
                    && satisfied == EvidencePurposeV1::ClaimedSupportGate)
        }
        ConsumptionEvidenceProvenanceV1::ReviewedCharacterization { .. } => {
            requested == EvidencePurposeV1::ProviderPathAdvisory
                && satisfied == EvidencePurposeV1::Characterization
        }
    }
}

impl EquivalentWorkStateV1 {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn validate(&self) -> Result<(), BoxError> {
        for (reservation_id, reservation) in &self.reservations {
            reservation.validate()?;
            let expected_key = equivalent_work_key(
                &reservation.case_execution,
                reservation.evidence_purpose,
                &reservation.freshness_bucket,
            )?;
            let expected_id = format!(
                "reservation-{}",
                admission_hash(
                    "equivalent-work-reservation-id",
                    &reservation.admission_attempt,
                )?
            );
            let completed = self.completed.contains_key(reservation_id);
            let live = self
                .live_by_execution
                .get(&reservation.case_execution.sha256)
                == Some(reservation_id);
            if reservation_id != &reservation.reservation_id
                || reservation_id != &expected_id
                || reservation.equivalent_work_key != expected_key
                || completed == live
            {
                return Err("schedule admission: equivalent reservation key mismatch".into());
            }
        }
        for (execution, reservation_id) in &self.live_by_execution {
            let reservation = self
                .reservations
                .get(reservation_id)
                .ok_or("schedule admission: live equivalent work has no reservation")?;
            if execution != &reservation.case_execution.sha256
                || self.completed.contains_key(reservation_id)
            {
                return Err("schedule admission: live equivalent-work index diverged".into());
            }
        }
        for (reservation_id, evidence) in &self.completed {
            let reservation = self
                .reservations
                .get(reservation_id)
                .ok_or("schedule admission: completed evidence has no reservation")?;
            if reservation_id != &evidence.reservation_id
                || reservation.characterization_profile != evidence.characterization_profile
                || reservation.case_execution != evidence.case_execution
                || reservation.freshness_bucket != evidence.freshness_bucket
                || reservation.evidence_purpose != evidence.satisfied_purpose
                || evidence.terminal_at_ms < reservation.reserved_at_ms
                || !local_file::valid_sha256(&evidence.evidence_sha256)
                || (evidence.reusable
                    && matches!(
                        (evidence.satisfied_purpose, &evidence.provenance),
                        (EvidencePurposeV1::ManualDiagnostic, _)
                            | (
                                EvidencePurposeV1::Characterization,
                                ConsumptionEvidenceProvenanceV1::Ordinary
                            )
                    ))
                || (evidence.reusable
                    && evidence.expected_effective_identity != evidence.observed_effective_identity)
            {
                return Err(
                    "schedule admission: completed equivalent evidence is inconsistent".into(),
                );
            }
            if let ConsumptionEvidenceProvenanceV1::ReviewedCharacterization {
                terminal_at_ms,
                reviewed_at_ms,
                ..
            } = &evidence.provenance
            {
                if evidence.satisfied_purpose != EvidencePurposeV1::Characterization
                    || terminal_at_ms != &evidence.terminal_at_ms
                {
                    return Err(
                        "schedule admission: reviewed characterization provenance diverged".into(),
                    );
                }
                ConsumptionRecordV1 {
                    schema_version: 1,
                    consumption_id: "consumption-provenance-validation".into(),
                    equivalent_work_key: local_file::sha256_hex(
                        b"completed-evidence-provenance-validation",
                    ),
                    evidence_sha256: evidence.evidence_sha256.clone(),
                    requested_purpose: EvidencePurposeV1::ProviderPathAdvisory,
                    satisfied_purpose: EvidencePurposeV1::Characterization,
                    provenance: evidence.provenance.clone(),
                    characterization_profile: evidence.characterization_profile.clone(),
                    case_execution: evidence.case_execution.clone(),
                    admission_attempt: reservation.admission_attempt.clone(),
                    authority: reservation.authority.clone(),
                    consumed_at_ms: *reviewed_at_ms,
                }
                .validate()?;
            }
        }
        for (consumption_id, consumption) in &self.consumptions {
            consumption.validate()?;
            let expected_id = format!(
                "consumption-{}",
                admission_hash(
                    "equivalent-work-consumption-id",
                    &(
                        &consumption.admission_attempt,
                        &consumption.equivalent_work_key,
                        &consumption.evidence_sha256,
                    ),
                )?
            );
            let evidence_matches = self.completed.values().any(|evidence| {
                evidence.evidence_sha256 == consumption.evidence_sha256
                    && evidence.characterization_profile == consumption.characterization_profile
                    && evidence.case_execution == consumption.case_execution
                    && evidence.satisfied_purpose == consumption.satisfied_purpose
                    && evidence.provenance == consumption.provenance
                    && equivalent_work_key(
                        &evidence.case_execution,
                        consumption.requested_purpose,
                        &evidence.freshness_bucket,
                    )
                    .is_ok_and(|key| key == consumption.equivalent_work_key)
            });
            if consumption_id != &consumption.consumption_id
                || consumption_id != &expected_id
                || !evidence_matches
            {
                return Err("schedule admission: consumption key mismatch".into());
            }
        }
        Ok(())
    }

    fn eligible_evidence(
        &self,
        identities: &DerivedAdmissionIdentitiesV1,
    ) -> Option<&CompletedEquivalentEvidenceV1> {
        self.completed
            .values()
            .filter(|evidence| {
                evidence.reusable
                    && evidence.characterization_profile == identities.characterization_profile
                    && evidence.case_execution == identities.case_execution.fingerprint
                    && evidence.freshness_bucket == identities.freshness_bucket
                    && evidence.expected_effective_identity == evidence.observed_effective_identity
                    && purpose_satisfies(
                        identities.evidence_purpose,
                        evidence.satisfied_purpose,
                        &evidence.provenance,
                    )
            })
            .max_by(|left, right| {
                (left.terminal_at_ms, &left.evidence_sha256)
                    .cmp(&(right.terminal_at_ms, &right.evidence_sha256))
            })
    }

    pub(super) fn reserve_or_reuse(
        &mut self,
        identities: &DerivedAdmissionIdentitiesV1,
        authority: AdmissionAuthorityV1,
        reserved_at_ms: i64,
    ) -> Result<EquivalentWorkDecisionV1, BoxError> {
        let mut candidate = self.clone();
        let decision =
            candidate.reserve_or_reuse_in_place(identities, authority, reserved_at_ms)?;
        *self = candidate;
        Ok(decision)
    }

    fn reserve_or_reuse_in_place(
        &mut self,
        identities: &DerivedAdmissionIdentitiesV1,
        authority: AdmissionAuthorityV1,
        reserved_at_ms: i64,
    ) -> Result<EquivalentWorkDecisionV1, BoxError> {
        self.validate()?;
        if reserved_at_ms <= 0 || authority != identities.admission_attempt.input.authority {
            return Err("schedule admission: equivalent-work authority/time mismatch".into());
        }
        if let Some(evidence) = self.eligible_evidence(identities).cloned() {
            let consumption_hash = admission_hash(
                "equivalent-work-consumption-id",
                &(
                    &identities.admission_attempt.fingerprint,
                    &identities.equivalent_work_key,
                    &evidence.evidence_sha256,
                ),
            )?;
            let consumption_id = format!("consumption-{consumption_hash}");
            if let Some(existing) = self.consumptions.get(&consumption_id) {
                return Ok(EquivalentWorkDecisionV1::Reused(existing.clone()));
            }
            let consumption = ConsumptionRecordV1 {
                schema_version: 1,
                consumption_id: consumption_id.clone(),
                equivalent_work_key: identities.equivalent_work_key.clone(),
                evidence_sha256: evidence.evidence_sha256,
                requested_purpose: identities.evidence_purpose,
                satisfied_purpose: evidence.satisfied_purpose,
                provenance: evidence.provenance,
                characterization_profile: identities.characterization_profile.clone(),
                case_execution: identities.case_execution.fingerprint.clone(),
                admission_attempt: identities.admission_attempt.fingerprint.clone(),
                authority,
                consumed_at_ms: reserved_at_ms,
            };
            consumption.validate()?;
            self.consumptions
                .insert(consumption_id, consumption.clone());
            self.validate()?;
            return Ok(EquivalentWorkDecisionV1::Reused(consumption));
        }
        let execution_key = identities.case_execution.fingerprint.sha256.clone();
        if self.live_by_execution.contains_key(&execution_key) {
            return Err(
                "schedule admission: equivalent work already has a live reservation".into(),
            );
        }
        let reservation_hash = admission_hash(
            "equivalent-work-reservation-id",
            &identities.admission_attempt.fingerprint,
        )?;
        let reservation_id = format!("reservation-{reservation_hash}");
        if self.reservations.contains_key(&reservation_id) {
            return Err("schedule admission: admission attempt was already reserved".into());
        }
        let reservation = EquivalentWorkReservationV1 {
            schema_version: 1,
            reservation_id: reservation_id.clone(),
            equivalent_work_key: identities.equivalent_work_key.clone(),
            characterization_profile: identities.characterization_profile.clone(),
            case_execution: identities.case_execution.fingerprint.clone(),
            admission_attempt: identities.admission_attempt.fingerprint.clone(),
            evidence_purpose: identities.evidence_purpose,
            freshness_bucket: identities.freshness_bucket.clone(),
            authority,
            reserved_at_ms,
        };
        reservation.validate()?;
        self.live_by_execution
            .insert(execution_key, reservation_id.clone());
        self.reservations
            .insert(reservation_id, reservation.clone());
        self.validate()?;
        Ok(EquivalentWorkDecisionV1::Reserved(reservation))
    }

    pub(super) fn record_completed(
        &mut self,
        evidence: CompletedEquivalentEvidenceV1,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        candidate.record_completed_in_place(evidence)?;
        *self = candidate;
        Ok(())
    }

    fn record_completed_in_place(
        &mut self,
        evidence: CompletedEquivalentEvidenceV1,
    ) -> Result<(), BoxError> {
        self.validate()?;
        let reservation = self
            .reservations
            .get(&evidence.reservation_id)
            .ok_or("schedule admission: terminal evidence has no reservation")?;
        if self.completed.contains_key(&evidence.reservation_id)
            || self
                .live_by_execution
                .get(&reservation.case_execution.sha256)
                != Some(&evidence.reservation_id)
        {
            return Err("schedule admission: reservation is not live and terminalizable".into());
        }
        self.live_by_execution
            .remove(&reservation.case_execution.sha256);
        self.completed
            .insert(evidence.reservation_id.clone(), evidence);
        self.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct CharacterizationStateV1 {
    #[serde(default)]
    pub(super) records: BTreeMap<String, CharacterizationRecordV1>,
    #[serde(default)]
    pub(super) record_sha256s: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) current_by_profile: BTreeMap<String, String>,
}

impl CharacterizationStateV1 {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn validate(&self) -> Result<(), BoxError> {
        if self.records.len() != self.record_sha256s.len() {
            return Err("schedule admission: characterization hash index diverged".into());
        }
        for (characterization_id, record) in &self.records {
            record.validate()?;
            if characterization_id != &record.characterization_id {
                return Err("schedule admission: characterization record key mismatch".into());
            }
            let expected = characterization_record_sha256(record)?;
            if self.record_sha256s.get(characterization_id) != Some(&expected) {
                return Err("schedule admission: characterization record hash mismatch".into());
            }
        }
        for (profile_sha256, characterization_id) in &self.current_by_profile {
            let record = self
                .records
                .get(characterization_id)
                .ok_or("schedule admission: current characterization has no immutable record")?;
            if &record.characterization_profile.sha256 != profile_sha256
                || record.expected_effective_identity != record.observed_effective_identity
                || record.outcome == CharacterizationOutcomeV1::CharacterizationInconclusive
            {
                return Err(
                    "schedule admission: current characterization is not promotable".into(),
                );
            }
        }
        Ok(())
    }

    pub(super) fn record_terminal(
        &mut self,
        record: CharacterizationRecordV1,
    ) -> Result<String, BoxError> {
        let mut candidate = self.clone();
        let record_sha256 = candidate.record_terminal_in_place(record)?;
        *self = candidate;
        Ok(record_sha256)
    }

    fn record_terminal_in_place(
        &mut self,
        record: CharacterizationRecordV1,
    ) -> Result<String, BoxError> {
        self.validate()?;
        record.validate()?;
        let record_sha256 = characterization_record_sha256(&record)?;
        if let Some(existing) = self.records.get(&record.characterization_id) {
            if existing == &record {
                return Ok(record_sha256);
            }
            return Err("schedule admission: characterization id cannot be rebound".into());
        }
        let promotable = record.expected_effective_identity == record.observed_effective_identity
            && record.outcome != CharacterizationOutcomeV1::CharacterizationInconclusive;
        if promotable
            && self
                .current_by_profile
                .contains_key(&record.characterization_profile.sha256)
        {
            return Err(
                "schedule admission: profile already has a current characterization".into(),
            );
        }
        let characterization_id = record.characterization_id.clone();
        let profile_sha256 = record.characterization_profile.sha256.clone();
        self.record_sha256s
            .insert(characterization_id.clone(), record_sha256.clone());
        self.records.insert(characterization_id.clone(), record);
        if promotable {
            self.current_by_profile
                .insert(profile_sha256, characterization_id);
        }
        self.validate()?;
        Ok(record_sha256)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ConfirmationLifecycleV1 {
    Available,
    Consumed {
        confirmation_admission: FingerprintV1,
        consumed_at_ms: i64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfirmationAuthorizationV1 {
    pub(super) schema_version: u16,
    pub(super) confirmation_id: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) source_admission: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) evidence_purpose: EvidencePurposeV1,
    pub(super) equivalent_work_key: String,
    pub(super) freshness_bucket: String,
    pub(super) source_evidence_sha256: String,
    pub(super) failure_kind: FailureKindV1,
    pub(super) typed_code: String,
    pub(super) source_window_id: String,
    pub(super) next_window_id: String,
    pub(super) repeat_nonce: String,
    pub(super) authorized_at_ms: i64,
    pub(super) lifecycle: ConfirmationLifecycleV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FailureObservationV1 {
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) evidence_sha256: String,
    pub(super) failure_kind: FailureKindV1,
    pub(super) typed_code: String,
    pub(super) window_id: String,
    pub(super) observed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlStateV1 {
    #[serde(default)]
    pub(super) hold_openings: BTreeMap<String, SafetyHoldV1>,
    #[serde(default)]
    pub(super) hold_clearances: BTreeMap<String, SafetyHoldV1>,
    #[serde(default)]
    pub(super) active_hold_by_execution: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) quarantine_openings: BTreeMap<String, QuarantineV1>,
    #[serde(default)]
    pub(super) quarantine_closures: BTreeMap<String, QuarantineV1>,
    #[serde(default)]
    pub(super) active_quarantine_by_profile: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) failures: BTreeMap<String, FailureDispositionV1>,
    #[serde(default)]
    pub(super) confirmations: BTreeMap<String, ConfirmationAuthorizationV1>,
    #[serde(default)]
    pub(super) active_confirmation_by_execution: BTreeMap<String, String>,
}

fn validate_confirmation(value: &ConfirmationAuthorizationV1) -> Result<(), BoxError> {
    if value.schema_version != 1 || value.authorized_at_ms <= 0 {
        return Err("schedule admission: confirmation version/time is invalid".into());
    }
    for (label, value) in [
        ("confirmation id", value.confirmation_id.as_str()),
        (
            "confirmation source window",
            value.source_window_id.as_str(),
        ),
        ("confirmation next window", value.next_window_id.as_str()),
        ("confirmation repeat nonce", value.repeat_nonce.as_str()),
        ("confirmation typed code", value.typed_code.as_str()),
        (
            "confirmation freshness bucket",
            value.freshness_bucket.as_str(),
        ),
    ] {
        stable_id(label, value)?;
    }
    if value.source_window_id == value.next_window_id
        || !local_file::valid_sha256(&value.source_evidence_sha256)
        || !local_file::valid_sha256(&value.equivalent_work_key)
        || matches!(
            value.failure_kind,
            FailureKindV1::TypedImmutable | FailureKindV1::CandidateUnknown
        )
    {
        return Err(
            "schedule admission: confirmation does not bind a transient next window".into(),
        );
    }
    if !matches!(value.authority, AdmissionAuthorityV1::StandingGrant(_)) {
        return Err("schedule admission: confirmation requires standing-grant authority".into());
    }
    for fingerprint in [
        &value.characterization_profile,
        &value.case_execution,
        &value.source_admission,
    ] {
        if fingerprint.schema_version != 1 || !local_file::valid_sha256(&fingerprint.sha256) {
            return Err("schedule admission: confirmation fingerprint is invalid".into());
        }
    }
    if equivalent_work_key(
        &value.case_execution,
        value.evidence_purpose,
        &value.freshness_bucket,
    )? != value.equivalent_work_key
    {
        return Err("schedule admission: confirmation equivalent-work identity drifted".into());
    }
    if let ConfirmationLifecycleV1::Consumed {
        confirmation_admission,
        consumed_at_ms,
    } = &value.lifecycle
    {
        if confirmation_admission.schema_version != 1
            || !local_file::valid_sha256(&confirmation_admission.sha256)
            || *consumed_at_ms < value.authorized_at_ms
        {
            return Err("schedule admission: consumed confirmation is invalid".into());
        }
    }
    Ok(())
}

fn quarantine_opening_sha256(value: &QuarantineV1) -> Result<String, BoxError> {
    match value {
        QuarantineV1::Open { .. } => admission_hash("quarantine-opening", value),
        QuarantineV1::Closed { .. } => {
            Err("schedule admission: quarantine opening must be open".into())
        }
    }
}

impl ControlStateV1 {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn validate(&self) -> Result<(), BoxError> {
        for (hold_id, opening) in &self.hold_openings {
            opening.validate()?;
            if hold_id != &opening.hold_id || opening.lifecycle != HoldLifecycleV1::Active {
                return Err("schedule admission: safety-hold opening index diverged".into());
            }
        }
        for (hold_id, clearance) in &self.hold_clearances {
            clearance.validate()?;
            let opening = self
                .hold_openings
                .get(hold_id)
                .ok_or("schedule admission: safety-hold clearance has no opening")?;
            let HoldLifecycleV1::Cleared { opening_sha256, .. } = &clearance.lifecycle else {
                return Err("schedule admission: safety-hold clearance is not cleared".into());
            };
            if hold_id != &clearance.hold_id
                || opening.characterization_profile != clearance.characterization_profile
                || opening.case_execution != clearance.case_execution
                || opening.reason != clearance.reason
                || opening.created_at_ms != clearance.created_at_ms
                || opening_sha256 != &safety_hold_opening_sha256(opening)?
            {
                return Err(
                    "schedule admission: safety-hold clearance does not bind its opening".into(),
                );
            }
        }
        for (execution, hold_id) in &self.active_hold_by_execution {
            let opening = self
                .hold_openings
                .get(hold_id)
                .ok_or("schedule admission: active safety hold has no opening")?;
            if execution != &opening.case_execution.sha256
                || self.hold_clearances.contains_key(hold_id)
            {
                return Err("schedule admission: active safety-hold index diverged".into());
            }
        }
        for (hold_id, opening) in &self.hold_openings {
            let active = self
                .active_hold_by_execution
                .get(&opening.case_execution.sha256)
                == Some(hold_id);
            let cleared = self.hold_clearances.contains_key(hold_id);
            if active == cleared {
                return Err("schedule admission: safety-hold lifecycle is incomplete".into());
            }
        }
        for (quarantine_id, opening) in &self.quarantine_openings {
            opening.validate()?;
            let QuarantineV1::Open {
                quarantine_id: embedded,
                ..
            } = opening
            else {
                return Err("schedule admission: quarantine opening is not open".into());
            };
            if quarantine_id != embedded {
                return Err("schedule admission: quarantine opening key mismatch".into());
            }
        }
        for (quarantine_id, closure) in &self.quarantine_closures {
            closure.validate()?;
            let opening = self
                .quarantine_openings
                .get(quarantine_id)
                .ok_or("schedule admission: quarantine closure has no opening")?;
            let (
                QuarantineV1::Open {
                    quarantine_id: opening_id,
                    profile: opening_profile,
                    created_at_ms: opening_created,
                    ..
                },
                QuarantineV1::Closed {
                    quarantine_id: closure_id,
                    profile: closure_profile,
                    opening_sha256,
                    created_at_ms: closure_created,
                    ..
                },
            ) = (opening, closure)
            else {
                return Err("schedule admission: quarantine history has invalid variants".into());
            };
            if quarantine_id != opening_id
                || quarantine_id != closure_id
                || opening_profile != closure_profile
                || opening_created != closure_created
                || opening_sha256 != &quarantine_opening_sha256(opening)?
            {
                return Err(
                    "schedule admission: quarantine closure does not bind its opening".into(),
                );
            }
        }
        for (profile, quarantine_id) in &self.active_quarantine_by_profile {
            let opening = self
                .quarantine_openings
                .get(quarantine_id)
                .ok_or("schedule admission: active quarantine has no opening")?;
            let QuarantineV1::Open {
                profile: opening_profile,
                ..
            } = opening
            else {
                return Err("schedule admission: active quarantine is not open".into());
            };
            if profile != &opening_profile.sha256
                || self.quarantine_closures.contains_key(quarantine_id)
            {
                return Err("schedule admission: active quarantine index diverged".into());
            }
        }
        for (quarantine_id, opening) in &self.quarantine_openings {
            let QuarantineV1::Open { profile, .. } = opening else {
                return Err("schedule admission: quarantine opening is not open".into());
            };
            let active =
                self.active_quarantine_by_profile.get(&profile.sha256) == Some(quarantine_id);
            let closed = self.quarantine_closures.contains_key(quarantine_id);
            if active == closed {
                return Err("schedule admission: quarantine lifecycle is incomplete".into());
            }
        }
        for (execution, disposition) in &self.failures {
            disposition.validate()?;
            if execution != &disposition.case_execution.sha256 {
                return Err("schedule admission: failure disposition key mismatch".into());
            }
        }
        let mut repeat_nonces = std::collections::BTreeSet::new();
        for (confirmation_id, confirmation) in &self.confirmations {
            validate_confirmation(confirmation)?;
            if confirmation_id != &confirmation.confirmation_id
                || !repeat_nonces.insert(confirmation.repeat_nonce.as_str())
            {
                return Err("schedule admission: confirmation identity/nonce is not unique".into());
            }
        }
        for (execution, confirmation_id) in &self.active_confirmation_by_execution {
            let confirmation = self
                .confirmations
                .get(confirmation_id)
                .ok_or("schedule admission: active confirmation has no authorization")?;
            let failure = self
                .failures
                .get(execution)
                .ok_or("schedule admission: active confirmation has no due failure")?;
            if execution != &confirmation.case_execution.sha256
                || confirmation.lifecycle != ConfirmationLifecycleV1::Available
                || failure.action != FailureActionV1::ConfirmationDue
                || failure.characterization_profile != confirmation.characterization_profile
                || failure.case_execution != confirmation.case_execution
                || failure.evidence_sha256 != confirmation.source_evidence_sha256
                || failure.failure_kind != confirmation.failure_kind
                || failure.typed_code != confirmation.typed_code
            {
                return Err("schedule admission: active confirmation index diverged".into());
            }
        }
        for (confirmation_id, confirmation) in &self.confirmations {
            let active = self
                .active_confirmation_by_execution
                .get(&confirmation.case_execution.sha256)
                == Some(confirmation_id);
            if active != (confirmation.lifecycle == ConfirmationLifecycleV1::Available) {
                return Err("schedule admission: confirmation lifecycle is incomplete".into());
            }
        }
        Ok(())
    }

    pub(super) fn open_safety_hold(
        &mut self,
        profile: FingerprintV1,
        execution: FingerprintV1,
        reason: HoldReasonV1,
        created_at_ms: i64,
    ) -> Result<SafetyHoldV1, BoxError> {
        let mut candidate = self.clone();
        let hold =
            candidate.open_safety_hold_in_place(profile, execution, reason, created_at_ms)?;
        *self = candidate;
        Ok(hold)
    }

    fn open_safety_hold_in_place(
        &mut self,
        profile: FingerprintV1,
        execution: FingerprintV1,
        reason: HoldReasonV1,
        created_at_ms: i64,
    ) -> Result<SafetyHoldV1, BoxError> {
        self.validate()?;
        let hash = admission_hash(
            "safety-hold-id",
            &(&profile, &execution, reason, created_at_ms),
        )?;
        let hold = SafetyHoldV1 {
            schema_version: 1,
            hold_id: format!("hold-{hash}"),
            characterization_profile: profile,
            case_execution: execution,
            reason,
            created_at_ms,
            lifecycle: HoldLifecycleV1::Active,
        };
        hold.validate()?;
        if let Some(existing_id) = self
            .active_hold_by_execution
            .get(&hold.case_execution.sha256)
        {
            if self.hold_openings.get(existing_id) == Some(&hold) {
                return Ok(hold);
            }
            return Err("schedule admission: execution already has an active safety hold".into());
        }
        self.active_hold_by_execution
            .insert(hold.case_execution.sha256.clone(), hold.hold_id.clone());
        self.hold_openings
            .insert(hold.hold_id.clone(), hold.clone());
        self.validate()?;
        Ok(hold)
    }

    pub(super) fn clear_safety_hold(
        &mut self,
        execution: &FingerprintV1,
        clearance_action_id: String,
        cleared_at_ms: i64,
        operator: String,
        reason: String,
    ) -> Result<SafetyHoldV1, BoxError> {
        let mut candidate = self.clone();
        let clearance = candidate.clear_safety_hold_in_place(
            execution,
            clearance_action_id,
            cleared_at_ms,
            operator,
            reason,
        )?;
        *self = candidate;
        Ok(clearance)
    }

    fn clear_safety_hold_in_place(
        &mut self,
        execution: &FingerprintV1,
        clearance_action_id: String,
        cleared_at_ms: i64,
        operator: String,
        reason: String,
    ) -> Result<SafetyHoldV1, BoxError> {
        self.validate()?;
        let hold_id = self
            .active_hold_by_execution
            .get(&execution.sha256)
            .ok_or("schedule admission: execution has no active safety hold")?
            .clone();
        let opening = self
            .hold_openings
            .get(&hold_id)
            .ok_or("schedule admission: active safety-hold opening disappeared")?;
        let opening_sha256 = safety_hold_opening_sha256(opening)?;
        let clearance_action_sha256 = safety_hold_clearance_action_sha256(
            &opening_sha256,
            &clearance_action_id,
            cleared_at_ms,
            &operator,
            &reason,
        )?;
        let mut clearance = opening.clone();
        clearance.lifecycle = HoldLifecycleV1::Cleared {
            opening_sha256,
            clearance_action_id,
            clearance_action_sha256,
            cleared_at_ms,
            operator,
            reason,
        };
        clearance.validate()?;
        self.active_hold_by_execution.remove(&execution.sha256);
        self.hold_clearances.insert(hold_id, clearance.clone());
        self.validate()?;
        Ok(clearance)
    }

    pub(super) fn open_quarantine(
        &mut self,
        profile: FingerprintV1,
        operator: String,
        reason: String,
        created_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<QuarantineV1, BoxError> {
        let mut candidate = self.clone();
        let quarantine = candidate.open_quarantine_in_place(
            profile,
            operator,
            reason,
            created_at_ms,
            expires_at_ms,
        )?;
        *self = candidate;
        Ok(quarantine)
    }

    fn open_quarantine_in_place(
        &mut self,
        profile: FingerprintV1,
        operator: String,
        reason: String,
        created_at_ms: i64,
        expires_at_ms: i64,
    ) -> Result<QuarantineV1, BoxError> {
        self.validate()?;
        let hash = admission_hash(
            "quarantine-id",
            &(&profile, &operator, &reason, created_at_ms, expires_at_ms),
        )?;
        let quarantine = QuarantineV1::Open {
            schema_version: 1,
            quarantine_id: format!("quarantine-{hash}"),
            profile,
            operator,
            reason,
            created_at_ms,
            expires_at_ms,
        };
        quarantine.validate()?;
        let QuarantineV1::Open {
            quarantine_id,
            profile,
            ..
        } = &quarantine
        else {
            unreachable!("constructed open quarantine")
        };
        if let Some(existing_id) = self.active_quarantine_by_profile.get(&profile.sha256) {
            if self.quarantine_openings.get(existing_id) == Some(&quarantine) {
                return Ok(quarantine);
            }
            return Err("schedule admission: profile already has an active quarantine".into());
        }
        self.active_quarantine_by_profile
            .insert(profile.sha256.clone(), quarantine_id.clone());
        self.quarantine_openings
            .insert(quarantine_id.clone(), quarantine.clone());
        self.validate()?;
        Ok(quarantine)
    }

    pub(super) fn close_quarantine(
        &mut self,
        profile: &FingerprintV1,
        operator: String,
        reason: String,
        closed_at_ms: i64,
    ) -> Result<QuarantineV1, BoxError> {
        let mut candidate = self.clone();
        let closure =
            candidate.close_quarantine_in_place(profile, operator, reason, closed_at_ms)?;
        *self = candidate;
        Ok(closure)
    }

    fn close_quarantine_in_place(
        &mut self,
        profile: &FingerprintV1,
        operator: String,
        reason: String,
        closed_at_ms: i64,
    ) -> Result<QuarantineV1, BoxError> {
        self.validate()?;
        let quarantine_id = self
            .active_quarantine_by_profile
            .get(&profile.sha256)
            .ok_or("schedule admission: profile has no active quarantine")?
            .clone();
        let opening = self
            .quarantine_openings
            .get(&quarantine_id)
            .ok_or("schedule admission: active quarantine opening disappeared")?;
        let QuarantineV1::Open {
            schema_version,
            profile,
            created_at_ms,
            ..
        } = opening
        else {
            return Err("schedule admission: active quarantine is not open".into());
        };
        let closure = QuarantineV1::Closed {
            schema_version: *schema_version,
            quarantine_id: quarantine_id.clone(),
            profile: profile.clone(),
            opening_sha256: quarantine_opening_sha256(opening)?,
            operator,
            reason,
            created_at_ms: *created_at_ms,
            closed_at_ms,
        };
        closure.validate()?;
        self.active_quarantine_by_profile.remove(&profile.sha256);
        self.quarantine_closures
            .insert(quarantine_id, closure.clone());
        self.validate()?;
        Ok(closure)
    }

    pub(super) fn is_quarantined(&self, profile: &FingerprintV1) -> bool {
        self.active_quarantine_by_profile
            .contains_key(&profile.sha256)
    }

    pub(super) fn observe_failure(
        &mut self,
        identities: &DerivedAdmissionIdentitiesV1,
        observation: FailureObservationV1,
    ) -> Result<FailureDispositionV1, BoxError> {
        let mut candidate = self.clone();
        let disposition = candidate.observe_failure_in_place(identities, observation, None)?;
        *self = candidate;
        Ok(disposition)
    }

    fn observe_failure_in_place(
        &mut self,
        identities: &DerivedAdmissionIdentitiesV1,
        observation: FailureObservationV1,
        confirmation_id: Option<&str>,
    ) -> Result<FailureDispositionV1, BoxError> {
        self.validate()?;
        stable_id("failure window", &observation.window_id)?;
        stable_id("typed failure code", &observation.typed_code)?;
        if observation.observed_at_ms <= 0
            || !local_file::valid_sha256(&observation.evidence_sha256)
            || observation.characterization_profile.schema_version != 1
            || observation.case_execution.schema_version != 1
            || observation.admission_attempt.schema_version != 1
            || !local_file::valid_sha256(&observation.characterization_profile.sha256)
            || !local_file::valid_sha256(&observation.case_execution.sha256)
            || !local_file::valid_sha256(&observation.admission_attempt.sha256)
            || observation.characterization_profile != identities.characterization_profile
            || observation.case_execution != identities.case_execution.fingerprint
            || observation.admission_attempt != identities.admission_attempt.fingerprint
            || observation.window_id != identities.admission_attempt.input.trigger.window_id
        {
            return Err(
                "schedule admission: failure observation does not bind rederived identities".into(),
            );
        }
        let prior = self
            .failures
            .get(&observation.case_execution.sha256)
            .cloned();
        if confirmation_id.is_none()
            && prior
                .as_ref()
                .is_some_and(|prior| prior.action == FailureActionV1::ConfirmationDue)
        {
            return Err(
                "schedule admission: a due execution requires its confirmation authorization"
                    .into(),
            );
        }
        let identical = prior.as_ref().is_some_and(|prior| {
            prior.characterization_profile == observation.characterization_profile
                && prior.case_execution == observation.case_execution
                && prior.failure_kind == observation.failure_kind
                && prior.typed_code == observation.typed_code
        });
        let (occurrences, first_seen_ms, action) = match observation.failure_kind {
            FailureKindV1::TypedImmutable => (
                prior.as_ref().filter(|_| identical).map_or(1, |value| {
                    value.identical_complete_occurrences.saturating_add(1)
                }),
                prior
                    .as_ref()
                    .filter(|_| identical)
                    .map_or(observation.observed_at_ms, |value| value.first_seen_ms),
                FailureActionV1::Suppressed,
            ),
            FailureKindV1::CandidateUnknown => (
                prior.as_ref().filter(|_| identical).map_or(1, |value| {
                    value.identical_complete_occurrences.saturating_add(1)
                }),
                prior
                    .as_ref()
                    .filter(|_| identical)
                    .map_or(observation.observed_at_ms, |value| value.first_seen_ms),
                FailureActionV1::UnknownRetained,
            ),
            FailureKindV1::TypedTransient | FailureKindV1::UntypedTransient => {
                if confirmation_id.is_none() {
                    (
                        1,
                        observation.observed_at_ms,
                        FailureActionV1::ConfirmationDue,
                    )
                } else if identical {
                    let action = if observation.failure_kind == FailureKindV1::TypedTransient {
                        FailureActionV1::Suppressed
                    } else {
                        FailureActionV1::UnknownRetained
                    };
                    (
                        prior.as_ref().map_or(2, |value| {
                            value.identical_complete_occurrences.saturating_add(1)
                        }),
                        prior
                            .as_ref()
                            .map_or(observation.observed_at_ms, |value| value.first_seen_ms),
                        action,
                    )
                } else {
                    (
                        1,
                        observation.observed_at_ms,
                        FailureActionV1::UnknownRetained,
                    )
                }
            }
        };
        let disposition = FailureDispositionV1 {
            schema_version: 1,
            characterization_profile: observation.characterization_profile,
            case_execution: observation.case_execution,
            evidence_sha256: observation.evidence_sha256,
            failure_kind: if confirmation_id.is_some()
                && !identical
                && matches!(
                    observation.failure_kind,
                    FailureKindV1::TypedTransient | FailureKindV1::UntypedTransient
                ) {
                FailureKindV1::CandidateUnknown
            } else {
                observation.failure_kind
            },
            typed_code: if confirmation_id.is_some()
                && !identical
                && matches!(
                    observation.failure_kind,
                    FailureKindV1::TypedTransient | FailureKindV1::UntypedTransient
                ) {
                "confirmation-not-identical".into()
            } else {
                observation.typed_code
            },
            identical_complete_occurrences: occurrences,
            action,
            first_seen_ms,
            last_seen_ms: observation.observed_at_ms,
        };
        disposition.validate()?;
        self.failures.insert(
            disposition.case_execution.sha256.clone(),
            disposition.clone(),
        );
        self.validate()?;
        Ok(disposition)
    }

    pub(super) fn authorize_confirmation(
        &mut self,
        identities: &DerivedAdmissionIdentitiesV1,
        source_window_id: String,
        next_window_id: String,
        repeat_nonce: String,
        confirmation_allowance: u8,
        authorized_at_ms: i64,
    ) -> Result<ConfirmationAuthorizationV1, BoxError> {
        let mut candidate = self.clone();
        let confirmation = candidate.authorize_confirmation_in_place(
            identities,
            source_window_id,
            next_window_id,
            repeat_nonce,
            confirmation_allowance,
            authorized_at_ms,
        )?;
        *self = candidate;
        Ok(confirmation)
    }

    fn authorize_confirmation_in_place(
        &mut self,
        identities: &DerivedAdmissionIdentitiesV1,
        source_window_id: String,
        next_window_id: String,
        repeat_nonce: String,
        confirmation_allowance: u8,
        authorized_at_ms: i64,
    ) -> Result<ConfirmationAuthorizationV1, BoxError> {
        self.validate()?;
        if confirmation_allowance != 1 {
            return Err("schedule admission: standing grant has no confirmation allowance".into());
        }
        let failure = self
            .failures
            .get(&identities.case_execution.fingerprint.sha256)
            .ok_or("schedule admission: confirmation has no prior failure")?;
        if !matches!(
            identities.admission_attempt.input.authority,
            AdmissionAuthorityV1::StandingGrant(_)
        ) || failure.action != FailureActionV1::ConfirmationDue
            || failure.characterization_profile != identities.characterization_profile
            || source_window_id != identities.admission_attempt.input.trigger.window_id
            || self
                .active_confirmation_by_execution
                .contains_key(&failure.case_execution.sha256)
            || self
                .confirmations
                .values()
                .any(|value| value.repeat_nonce == repeat_nonce)
        {
            return Err(
                "schedule admission: confirmation is not uniquely due and authorized".into(),
            );
        }
        let hash = admission_hash(
            "confirmation-id",
            &(
                &failure.case_execution,
                &failure.evidence_sha256,
                &source_window_id,
                &next_window_id,
                &repeat_nonce,
            ),
        )?;
        let confirmation = ConfirmationAuthorizationV1 {
            schema_version: 1,
            confirmation_id: format!("confirmation-{hash}"),
            characterization_profile: failure.characterization_profile.clone(),
            case_execution: failure.case_execution.clone(),
            source_admission: identities.admission_attempt.fingerprint.clone(),
            authority: identities.admission_attempt.input.authority.clone(),
            evidence_purpose: identities.evidence_purpose,
            equivalent_work_key: identities.equivalent_work_key.clone(),
            freshness_bucket: identities.freshness_bucket.clone(),
            source_evidence_sha256: failure.evidence_sha256.clone(),
            failure_kind: failure.failure_kind,
            typed_code: failure.typed_code.clone(),
            source_window_id,
            next_window_id,
            repeat_nonce,
            authorized_at_ms,
            lifecycle: ConfirmationLifecycleV1::Available,
        };
        validate_confirmation(&confirmation)?;
        self.active_confirmation_by_execution.insert(
            confirmation.case_execution.sha256.clone(),
            confirmation.confirmation_id.clone(),
        );
        self.confirmations
            .insert(confirmation.confirmation_id.clone(), confirmation.clone());
        self.validate()?;
        Ok(confirmation)
    }

    fn consume_confirmation(
        &mut self,
        confirmation_id: &str,
        identities: &DerivedAdmissionIdentitiesV1,
        observed_at_ms: i64,
    ) -> Result<(), BoxError> {
        let confirmation = self
            .confirmations
            .get_mut(confirmation_id)
            .ok_or("schedule admission: confirmation authorization does not exist")?;
        if confirmation.lifecycle != ConfirmationLifecycleV1::Available
            || confirmation.characterization_profile != identities.characterization_profile
            || confirmation.case_execution != identities.case_execution.fingerprint
            || confirmation.authority != identities.admission_attempt.input.authority
            || confirmation.evidence_purpose != identities.evidence_purpose
            || confirmation.equivalent_work_key != identities.equivalent_work_key
            || confirmation.freshness_bucket != identities.freshness_bucket
            || confirmation.next_window_id != identities.admission_attempt.input.trigger.window_id
            || identities.admission_attempt.input.trigger.repeat_nonce
                != (OptionalStableIdV1::StableId {
                    value: confirmation.repeat_nonce.clone(),
                })
        {
            return Err(
                "schedule admission: confirmation attempt does not match its authorization".into(),
            );
        }
        confirmation.lifecycle = ConfirmationLifecycleV1::Consumed {
            confirmation_admission: identities.admission_attempt.fingerprint.clone(),
            consumed_at_ms: observed_at_ms,
        };
        self.active_confirmation_by_execution
            .remove(&confirmation.case_execution.sha256);
        Ok(())
    }

    pub(super) fn observe_confirmation_failure(
        &mut self,
        confirmation_id: &str,
        identities: &DerivedAdmissionIdentitiesV1,
        observation: FailureObservationV1,
    ) -> Result<FailureDispositionV1, BoxError> {
        let mut candidate = self.clone();
        candidate.validate()?;
        candidate.consume_confirmation(confirmation_id, identities, observation.observed_at_ms)?;
        let disposition =
            candidate.observe_failure_in_place(identities, observation, Some(confirmation_id))?;
        *self = candidate;
        Ok(disposition)
    }

    pub(super) fn observe_confirmation_success(
        &mut self,
        confirmation_id: &str,
        identities: &DerivedAdmissionIdentitiesV1,
        evidence_sha256: String,
        observed_at_ms: i64,
    ) -> Result<FailureDispositionV1, BoxError> {
        let mut candidate = self.clone();
        candidate.validate()?;
        if !local_file::valid_sha256(&evidence_sha256) || observed_at_ms <= 0 {
            return Err("schedule admission: confirmation success evidence/time is invalid".into());
        }
        candidate.consume_confirmation(confirmation_id, identities, observed_at_ms)?;
        let prior = candidate
            .failures
            .get(&identities.case_execution.fingerprint.sha256)
            .ok_or("schedule admission: confirmation success has no prior failure")?
            .clone();
        let recovered = FailureDispositionV1 {
            evidence_sha256,
            action: FailureActionV1::Recovered,
            last_seen_ms: observed_at_ms,
            ..prior
        };
        recovered.validate()?;
        candidate
            .failures
            .insert(recovered.case_execution.sha256.clone(), recovered.clone());
        candidate.validate()?;
        *self = candidate;
        Ok(recovered)
    }

    pub(super) fn record_identity_mismatch(
        &mut self,
        identities: &DerivedAdmissionIdentitiesV1,
        observed_effective_identity: &EffectiveIdentityV1,
        evidence_sha256: String,
        observed_at_ms: i64,
    ) -> Result<(FailureDispositionV1, SafetyHoldV1), BoxError> {
        if observed_effective_identity
            == &identities.case_execution.input.expected_effective_identity
        {
            return Err("schedule admission: matching effective identity is not drift".into());
        }
        let mut candidate = self.clone();
        let disposition = candidate.observe_failure_in_place(
            identities,
            FailureObservationV1 {
                characterization_profile: identities.characterization_profile.clone(),
                case_execution: identities.case_execution.fingerprint.clone(),
                admission_attempt: identities.admission_attempt.fingerprint.clone(),
                evidence_sha256,
                failure_kind: FailureKindV1::CandidateUnknown,
                typed_code: "effective-identity-mismatch".into(),
                window_id: identities.admission_attempt.input.trigger.window_id.clone(),
                observed_at_ms,
            },
            None,
        )?;
        let hold = candidate.open_safety_hold_in_place(
            identities.characterization_profile.clone(),
            identities.case_execution.fingerprint.clone(),
            HoldReasonV1::IdentityDriftAfterEffect,
            observed_at_ms,
        )?;
        *self = candidate;
        Ok((disposition, hold))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};

    use crate::compatibility_schedule::{EffectCapsV1, EffectClassV1, FoundationProfileBindingV1};
    use crate::compatibility_schedule_authority::{
        derive_manual_admission, generate_claimed_support_characterization_source,
        generate_scheduled_execution_source, ManualAdmissionBindingsV1, ManualAdmissionOriginV1,
        ManualNonceSource,
    };
    use crate::compatibility_schedule_schema::{
        CandidateBinaryIdentityV1, CharacterizationOnceAuthorityV1, ExactExecutionBindingsV1,
        ExactExecutionTargetV1, GitObjectAlgorithmV1, GitObjectIdV1,
        ManualAcknowledgementAuthorityV1, OptionalGitObjectIdV1, OptionalSha256V1, OptionalTextV1,
        ProfileSourceKindV1, ProfileSourceRefV1, StandingGrantAuthorityV1, TriggerSourceV1,
    };

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    fn fingerprint(ch: char) -> FingerprintV1 {
        FingerprintV1 {
            schema_version: 1,
            sha256: digest(ch),
        }
    }

    fn sha1(ch: char) -> GitObjectIdV1 {
        GitObjectIdV1 {
            algorithm: GitObjectAlgorithmV1::Sha1,
            hex: ch.to_string().repeat(40),
        }
    }

    fn text(value: &str) -> OptionalTextV1 {
        OptionalTextV1::Text {
            value: value.into(),
        }
    }

    fn effective_identity(model: &str) -> EffectiveIdentityV1 {
        EffectiveIdentityV1 {
            model: model.into(),
            effort: text("low"),
            mode: OptionalTextV1::Absent,
        }
    }

    fn standing_authority(generation: u64) -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::StandingGrant(StandingGrantAuthorityV1 {
            grant_id: "grant-1".into(),
            generation,
            grant_sha256: digest(if generation == 1 { 'a' } else { 'b' }),
            characterization_id: "characterization-1".into(),
            characterization_sha256: digest('c'),
        })
    }

    fn one_shot_authority(entry: &str) -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::CharacterizationOnce(CharacterizationOnceAuthorityV1 {
            batch_authorization_id: "authorization-1".into(),
            batch_authorization_sha256: digest('d'),
            entry_id: entry.into(),
            generation: 1,
            entry_sha256: digest(if entry == "entry-1" { 'e' } else { 'f' }),
            consumption_nonce: format!("nonce-{entry}"),
        })
    }

    fn manual_authority(nonce: &str) -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::ManualAcknowledgement(ManualAcknowledgementAuthorityV1 {
            manual_admission_sha256: digest('9'),
            request_nonce: nonce.into(),
        })
    }

    fn daily_trigger(
        request: &str,
        window: &str,
        attempt: &str,
        repeat_nonce: OptionalStableIdV1,
    ) -> AdmissionTriggerIdentityV1 {
        AdmissionTriggerIdentityV1 {
            source: TriggerSourceV1::DailyLaunchd,
            kind: TriggerKindV1::Daily,
            request_id: request.into(),
            window_id: window.into(),
            attempt_id: attempt.into(),
            repeat_nonce,
        }
    }

    fn characterization_trigger(
        request: &str,
        window: &str,
        attempt: &str,
    ) -> AdmissionTriggerIdentityV1 {
        AdmissionTriggerIdentityV1 {
            source: TriggerSourceV1::ManualCharacterizationCli,
            kind: TriggerKindV1::ManualCharacterization,
            request_id: request.into(),
            window_id: window.into(),
            attempt_id: attempt.into(),
            repeat_nonce: OptionalStableIdV1::Absent,
        }
    }

    fn manual_trigger(nonce: &str, attempt: &str) -> AdmissionTriggerIdentityV1 {
        AdmissionTriggerIdentityV1 {
            source: TriggerSourceV1::ManualCompatibilityCli,
            kind: TriggerKindV1::ManualCompatibility,
            request_id: nonce.into(),
            window_id: "manual-window".into(),
            attempt_id: attempt.into(),
            repeat_nonce: OptionalStableIdV1::Absent,
        }
    }

    fn execution_input(profile: &FingerprintV1) -> CaseExecutionFingerprintInputV1 {
        let base = sha1('1');
        let head = sha1('2');
        CaseExecutionFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: profile.clone(),
            target: ExactExecutionTargetV1::TestMerge {
                repository: "shoedog/a2acp".into(),
                pull_request: 41,
                base_oid: base.clone(),
                head_oid: head.clone(),
                merge_oid: sha1('3'),
                merge_ref: "refs/pull/41/merge".into(),
                tree_oid: sha1('4'),
                ordered_parents: vec![base, head],
            },
            candidate: CandidateBinaryIdentityV1 {
                sha256: digest('5'),
                length_bytes: 1024,
                build_provenance_sha256: digest('6'),
            },
            bindings: ExactExecutionBindingsV1 {
                source_sha256: digest('7'),
                row_sha256: digest('8'),
                run_manifest_sha256: digest('9'),
                generated_config_sha256: digest('a'),
                pin_set_sha256: digest('b'),
                resolution_bundle: OptionalSha256V1::Sha256 { value: digest('c') },
                package_integrity_sha256: digest('d'),
                image_digest: OptionalSha256V1::Sha256 { value: digest('e') },
                base_image_digest: OptionalSha256V1::Sha256 { value: digest('f') },
                environment_sha256: digest('0'),
                prerequisites_sha256: digest('1'),
            },
            requested_identity: effective_identity("gpt-5.6-luna"),
            expected_effective_identity: effective_identity("gpt-5.6-luna"),
            actual_caps: EffectCapsV1 {
                timeout_secs: 60,
                max_tokens: 1000,
                max_cost_microusd: 1000,
                attempts: 1,
                retry_cap: 0,
                fallback_cap: 0,
            },
        }
    }

    fn derive(
        input: CaseExecutionFingerprintInputV1,
        authority: AdmissionAuthorityV1,
        trigger: AdmissionTriggerIdentityV1,
        purpose: EvidencePurposeV1,
        freshness: &str,
    ) -> DerivedAdmissionIdentitiesV1 {
        let profile = input.characterization_profile.clone();
        derive_admission_identities(
            &profile,
            input,
            authority,
            trigger,
            purpose,
            freshness.into(),
        )
        .unwrap()
    }

    fn ordinary_ids(
        input: &CaseExecutionFingerprintInputV1,
        request: &str,
        window: &str,
        attempt: &str,
    ) -> DerivedAdmissionIdentitiesV1 {
        derive(
            input.clone(),
            standing_authority(1),
            daily_trigger(request, window, attempt, OptionalStableIdV1::Absent),
            EvidencePurposeV1::ProviderPathAdvisory,
            "freshness-1",
        )
    }

    fn foundation_input(binding: &FoundationProfileBindingV1) -> CaseExecutionFingerprintInputV1 {
        let mut input = execution_input(&binding.characterization_profile);
        input.bindings.source_sha256 = binding.source.source_sha256.clone();
        input.bindings.row_sha256 = binding.source.row_sha256.clone();
        input.bindings.generated_config_sha256 = binding.exact_config_sha256.clone();
        input.requested_identity = binding.requested_identity.clone();
        input.expected_effective_identity = binding.expected_effective_identity.clone();
        input.actual_caps = binding.maximum_caps.clone();
        input
    }

    fn completed(
        reservation: &EquivalentWorkReservationV1,
        purpose: EvidencePurposeV1,
        provenance: ConsumptionEvidenceProvenanceV1,
        reusable: bool,
        evidence_ch: char,
        terminal_at_ms: i64,
    ) -> CompletedEquivalentEvidenceV1 {
        let effective_identity = effective_identity("gpt-5.6-luna");
        CompletedEquivalentEvidenceV1 {
            reservation_id: reservation.reservation_id.clone(),
            evidence_sha256: digest(evidence_ch),
            satisfied_purpose: purpose,
            freshness_bucket: reservation.freshness_bucket.clone(),
            characterization_profile: reservation.characterization_profile.clone(),
            case_execution: reservation.case_execution.clone(),
            expected_effective_identity: effective_identity.clone(),
            observed_effective_identity: effective_identity,
            provenance,
            reusable,
            terminal_at_ms,
        }
    }

    fn reserved(decision: EquivalentWorkDecisionV1) -> EquivalentWorkReservationV1 {
        let EquivalentWorkDecisionV1::Reserved(value) = decision else {
            panic!("expected reservation")
        };
        value
    }

    fn failure(
        identities: &DerivedAdmissionIdentitiesV1,
        kind: FailureKindV1,
        code: &str,
        evidence_ch: char,
        observed_at_ms: i64,
    ) -> FailureObservationV1 {
        FailureObservationV1 {
            characterization_profile: identities.characterization_profile.clone(),
            case_execution: identities.case_execution.fingerprint.clone(),
            admission_attempt: identities.admission_attempt.fingerprint.clone(),
            evidence_sha256: digest(evidence_ch),
            failure_kind: kind,
            typed_code: code.into(),
            window_id: identities.admission_attempt.input.trigger.window_id.clone(),
            observed_at_ms,
        }
    }

    #[test]
    fn final_fence_rederives_scheduled_claimed_support_and_manual_sources() {
        struct FixedNonce;
        impl ManualNonceSource for FixedNonce {
            fn fill(&self, output: &mut [u8]) -> Result<(), BoxError> {
                output.fill(7);
                Ok(())
            }
        }

        let foundation_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility");
        let foundation = load_schedule_foundation(&foundation_root).unwrap();

        let (scheduled_case, scheduled_binding) =
            foundation.scheduled_profiles.iter().next().unwrap();
        let scheduled_input = foundation_input(scheduled_binding);
        let scheduled_ids = derive(
            scheduled_input,
            standing_authority(1),
            daily_trigger(
                "request-scheduled",
                "window-scheduled",
                "attempt-scheduled",
                OptionalStableIdV1::Absent,
            ),
            scheduled_binding.evidence_purpose,
            "freshness-scheduled",
        );
        let scheduled = generate_scheduled_execution_source(
            &foundation_root,
            scheduled_case,
            scheduled_ids.case_execution.clone(),
            scheduled_ids.admission_attempt.clone(),
            scheduled_ids.admission_attempt.input.authority.clone(),
            TriggerKindV1::Daily,
        )
        .unwrap();
        assert_eq!(
            rederive_scheduled_identities(
                &foundation_root,
                &scheduled,
                "freshness-scheduled".into(),
            )
            .unwrap(),
            scheduled_ids
        );

        let (claimed_case, claimed_binding) =
            foundation.claimed_support_profiles.iter().next().unwrap();
        let claimed_input = foundation_input(claimed_binding);
        let claimed_ids = derive(
            claimed_input,
            one_shot_authority("entry-1"),
            characterization_trigger(
                "request-characterize",
                "window-characterize",
                "attempt-characterize",
            ),
            EvidencePurposeV1::Characterization,
            "freshness-characterize",
        );
        let claimed = generate_claimed_support_characterization_source(
            &foundation_root,
            claimed_case,
            claimed_ids.case_execution.clone(),
            claimed_ids.admission_attempt.clone(),
            claimed_ids.admission_attempt.input.authority.clone(),
        )
        .unwrap();
        assert_eq!(
            rederive_claimed_support_identities(
                &foundation_root,
                &claimed,
                "freshness-characterize".into(),
            )
            .unwrap(),
            claimed_ids
        );

        let manual_profile = fingerprint('2');
        let manual_input = execution_input(&manual_profile);
        let manual_execution = seal_case_execution_fingerprint(manual_input.clone()).unwrap();
        let manual = derive_manual_admission(
            ManualAdmissionOriginV1::DirectLocalCompatibilityCli,
            true,
            None,
            &FixedNonce,
            ManualAdmissionBindingsV1 {
                operator: "operator".into(),
                environment_owner: "wesleyjinks".into(),
                scheduler_binary_sha256: digest('3'),
                input_source_sha256: digest('4'),
                characterization_profile: manual_profile,
                case_execution: manual_execution.fingerprint,
                evidence_purpose: EvidencePurposeV1::ManualDiagnostic,
                freshness_bucket: "freshness-manual".into(),
                caps: manual_input.actual_caps.clone(),
                allowed_effects: vec![EffectClassV1::ProviderPrompt],
                issued_at_ms: 10,
                expires_at_ms: 20,
            },
        )
        .unwrap();
        let manual_trigger = manual_trigger(&manual.record.request_nonce, "manual-attempt");
        let manual_ids = rederive_manual_identities(&manual, manual_input, manual_trigger).unwrap();
        assert_eq!(
            manual_ids.characterization_profile,
            manual.record.characterization_profile
        );
        assert_eq!(
            manual_ids.case_execution.fingerprint,
            manual.record.case_execution
        );
        assert_eq!(
            manual_ids.admission_attempt.input.authority,
            manual.authority
        );
    }

    #[test]
    fn trigger_and_authority_change_only_admission_and_attempt_idempotency() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let base = ordinary_ids(&input, "request-1", "window-1", "attempt-1");
        for changed in [
            derive(
                input.clone(),
                standing_authority(1),
                daily_trigger(
                    "request-2",
                    "window-1",
                    "attempt-1",
                    OptionalStableIdV1::Absent,
                ),
                EvidencePurposeV1::ProviderPathAdvisory,
                "freshness-1",
            ),
            derive(
                input.clone(),
                standing_authority(1),
                daily_trigger(
                    "request-1",
                    "window-2",
                    "attempt-2",
                    OptionalStableIdV1::StableId {
                        value: "repeat-1".into(),
                    },
                ),
                EvidencePurposeV1::ProviderPathAdvisory,
                "freshness-1",
            ),
            derive(
                input.clone(),
                standing_authority(2),
                daily_trigger(
                    "request-1",
                    "window-1",
                    "attempt-1",
                    OptionalStableIdV1::Absent,
                ),
                EvidencePurposeV1::ProviderPathAdvisory,
                "freshness-1",
            ),
            derive(
                input.clone(),
                standing_authority(1),
                AdmissionTriggerIdentityV1 {
                    source: TriggerSourceV1::ScheduledMainCoalescer,
                    kind: TriggerKindV1::ScheduledMain,
                    request_id: "request-main".into(),
                    window_id: "window-main".into(),
                    attempt_id: "attempt-main".into(),
                    repeat_nonce: OptionalStableIdV1::Absent,
                },
                EvidencePurposeV1::ProviderPathAdvisory,
                "freshness-1",
            ),
            derive(
                input.clone(),
                manual_authority("manual-authority-change"),
                manual_trigger("manual-authority-change", "manual-attempt"),
                EvidencePurposeV1::ProviderPathAdvisory,
                "freshness-1",
            ),
        ] {
            assert_eq!(
                changed.characterization_profile,
                base.characterization_profile
            );
            assert_eq!(changed.case_execution, base.case_execution);
            assert_eq!(changed.equivalent_work_key, base.equivalent_work_key);
            assert_ne!(changed.admission_attempt, base.admission_attempt);
            assert_ne!(
                changed.attempt_idempotency_key,
                base.attempt_idempotency_key
            );
        }
    }

    #[test]
    fn every_exact_execution_binding_changes_execution_and_equivalence_only() {
        let profile = fingerprint('2');
        let base_input = execution_input(&profile);
        let base = ordinary_ids(&base_input, "request-1", "window-1", "attempt-1");
        let mut variants = Vec::<(&str, CaseExecutionFingerprintInputV1)>::new();
        macro_rules! changed {
            ($name:literal, $body:expr) => {{
                let mut value = base_input.clone();
                ($body)(&mut value);
                variants.push(($name, value));
            }};
        }
        changed!(
            "repository",
            |value: &mut CaseExecutionFingerprintInputV1| {
                let ExactExecutionTargetV1::TestMerge { repository, .. } = &mut value.target else {
                    unreachable!()
                };
                *repository = "shoedog/a2acp-next".into();
            }
        );
        changed!(
            "pull_request_and_ref",
            |value: &mut CaseExecutionFingerprintInputV1| {
                let ExactExecutionTargetV1::TestMerge {
                    pull_request,
                    merge_ref,
                    ..
                } = &mut value.target
                else {
                    unreachable!()
                };
                *pull_request = 42;
                *merge_ref = "refs/pull/42/merge".into();
            }
        );
        changed!(
            "base_and_parent_binding",
            |value: &mut CaseExecutionFingerprintInputV1| {
                let ExactExecutionTargetV1::TestMerge {
                    base_oid,
                    ordered_parents,
                    ..
                } = &mut value.target
                else {
                    unreachable!()
                };
                *base_oid = sha1('5');
                ordered_parents[0] = base_oid.clone();
            }
        );
        changed!(
            "head_and_parent_binding",
            |value: &mut CaseExecutionFingerprintInputV1| {
                let ExactExecutionTargetV1::TestMerge {
                    head_oid,
                    ordered_parents,
                    ..
                } = &mut value.target
                else {
                    unreachable!()
                };
                *head_oid = sha1('6');
                ordered_parents[1] = head_oid.clone();
            }
        );
        changed!(
            "merge_oid",
            |value: &mut CaseExecutionFingerprintInputV1| {
                let ExactExecutionTargetV1::TestMerge { merge_oid, .. } = &mut value.target else {
                    unreachable!()
                };
                *merge_oid = sha1('7');
            }
        );
        changed!("tree_oid", |value: &mut CaseExecutionFingerprintInputV1| {
            let ExactExecutionTargetV1::TestMerge { tree_oid, .. } = &mut value.target else {
                unreachable!()
            };
            *tree_oid = sha1('8');
        });
        changed!(
            "candidate_sha256",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.candidate.sha256 = digest('a');
            }
        );
        changed!(
            "candidate_length",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.candidate.length_bytes += 1;
            }
        );
        changed!(
            "candidate_build",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.candidate.build_provenance_sha256 = digest('b');
            }
        );
        macro_rules! binding_digest {
            ($name:literal, $field:ident, $ch:literal) => {
                changed!($name, |value: &mut CaseExecutionFingerprintInputV1| {
                    value.bindings.$field = digest($ch);
                });
            };
        }
        binding_digest!("source", source_sha256, '2');
        binding_digest!("row", row_sha256, '3');
        binding_digest!("run_manifest", run_manifest_sha256, '4');
        binding_digest!("generated_config", generated_config_sha256, '5');
        binding_digest!("pin_set", pin_set_sha256, '6');
        binding_digest!("package_integrity", package_integrity_sha256, '7');
        binding_digest!("environment", environment_sha256, '8');
        binding_digest!("prerequisites", prerequisites_sha256, '9');
        changed!(
            "resolution_bundle",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.bindings.resolution_bundle = OptionalSha256V1::Sha256 { value: digest('0') };
            }
        );
        changed!(
            "image_digest",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.bindings.image_digest = OptionalSha256V1::Sha256 { value: digest('1') };
            }
        );
        changed!(
            "base_image_digest",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.bindings.base_image_digest = OptionalSha256V1::Sha256 { value: digest('2') };
            }
        );
        changed!(
            "requested_model",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.requested_identity.model = "gpt-5.6-luna-next".into();
            }
        );
        changed!(
            "requested_effort",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.requested_identity.effort = text("medium");
            }
        );
        changed!(
            "requested_mode",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.requested_identity.mode = text("plan");
            }
        );
        changed!(
            "expected_model",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.expected_effective_identity.model = "gpt-5.6-luna-next".into();
            }
        );
        changed!(
            "expected_effort",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.expected_effective_identity.effort = text("medium");
            }
        );
        changed!(
            "expected_mode",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.expected_effective_identity.mode = text("plan");
            }
        );
        changed!(
            "timeout_cap",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.actual_caps.timeout_secs += 1;
            }
        );
        changed!(
            "token_cap",
            |value: &mut CaseExecutionFingerprintInputV1| {
                value.actual_caps.max_tokens += 1;
            }
        );
        changed!("cost_cap", |value: &mut CaseExecutionFingerprintInputV1| {
            value.actual_caps.max_cost_microusd += 1;
        });

        assert_eq!(variants.len(), 29);
        for (label, input) in variants {
            let changed = ordinary_ids(&input, "request-1", "window-1", "attempt-1");
            assert_eq!(
                changed.characterization_profile, base.characterization_profile,
                "{label}"
            );
            assert_ne!(changed.case_execution, base.case_execution, "{label}");
            assert_ne!(
                changed.equivalent_work_key, base.equivalent_work_key,
                "{label}"
            );
        }

        let mut snapshot_first = base_input.clone();
        snapshot_first.target = ExactExecutionTargetV1::RepositorySnapshot {
            repository: "shoedog/a2acp".into(),
            head_oid: sha1('1'),
            tree_oid: sha1('2'),
            range_start_exclusive: OptionalGitObjectIdV1::ObjectId { value: sha1('3') },
        };
        let mut snapshot_second = snapshot_first.clone();
        let ExactExecutionTargetV1::RepositorySnapshot {
            range_start_exclusive,
            ..
        } = &mut snapshot_second.target
        else {
            unreachable!()
        };
        *range_start_exclusive = OptionalGitObjectIdV1::ObjectId { value: sha1('4') };
        let first_snapshot = ordinary_ids(
            &snapshot_first,
            "request-snapshot",
            "window-snapshot",
            "attempt-snapshot",
        );
        let second_snapshot = ordinary_ids(
            &snapshot_second,
            "request-snapshot",
            "window-snapshot",
            "attempt-snapshot",
        );
        assert_eq!(
            first_snapshot.characterization_profile,
            second_snapshot.characterization_profile
        );
        assert_ne!(
            first_snapshot.case_execution,
            second_snapshot.case_execution
        );
        assert_ne!(
            first_snapshot.equivalent_work_key,
            second_snapshot.equivalent_work_key
        );

        let mut noncanonical = base_input;
        let ExactExecutionTargetV1::TestMerge {
            ordered_parents, ..
        } = &mut noncanonical.target
        else {
            unreachable!()
        };
        ordered_parents.swap(0, 1);
        assert!(seal_case_execution_fingerprint(noncanonical).is_err());
    }

    #[test]
    fn equivalent_work_refuses_live_duplicates_and_reuses_stronger_terminal_evidence() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let first = derive(
            input.clone(),
            standing_authority(1),
            daily_trigger(
                "request-1",
                "window-1",
                "attempt-1",
                OptionalStableIdV1::Absent,
            ),
            EvidencePurposeV1::ClaimedSupportGate,
            "freshness-1",
        );
        let duplicate = derive(
            input.clone(),
            standing_authority(2),
            daily_trigger(
                "request-2",
                "window-1",
                "attempt-2",
                OptionalStableIdV1::Absent,
            ),
            EvidencePurposeV1::ClaimedSupportGate,
            "freshness-1",
        );
        let mut state = EquivalentWorkStateV1::new();
        let reservation = reserved(
            state
                .reserve_or_reuse(&first, first.admission_attempt.input.authority.clone(), 10)
                .unwrap(),
        );
        let before = state.clone();
        assert!(state
            .reserve_or_reuse(
                &duplicate,
                duplicate.admission_attempt.input.authority.clone(),
                11,
            )
            .is_err());
        assert_eq!(state, before);

        state
            .record_completed(completed(
                &reservation,
                EvidencePurposeV1::ClaimedSupportGate,
                ConsumptionEvidenceProvenanceV1::Ordinary,
                true,
                '3',
                12,
            ))
            .unwrap();
        let advisory = ordinary_ids(&input, "request-3", "window-2", "attempt-3");
        let decision = state
            .reserve_or_reuse(
                &advisory,
                advisory.admission_attempt.input.authority.clone(),
                20,
            )
            .unwrap();
        let EquivalentWorkDecisionV1::Reused(consumption) = decision else {
            panic!("stronger completed evidence should be consumed")
        };
        assert_eq!(
            consumption.satisfied_purpose,
            EvidencePurposeV1::ClaimedSupportGate
        );
        assert_eq!(state.consumptions.len(), 1);
        state.validate().unwrap();
    }

    #[test]
    fn reviewed_characterization_may_satisfy_advisory_but_characterization_and_manual_never_reuse()
    {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let characterization = derive(
            input.clone(),
            one_shot_authority("entry-1"),
            characterization_trigger("characterize-1", "window-1", "attempt-1"),
            EvidencePurposeV1::Characterization,
            "freshness-1",
        );
        let mut state = EquivalentWorkStateV1::new();
        let reservation = reserved(
            state
                .reserve_or_reuse(
                    &characterization,
                    characterization.admission_attempt.input.authority.clone(),
                    10,
                )
                .unwrap(),
        );
        state
            .record_completed(completed(
                &reservation,
                EvidencePurposeV1::Characterization,
                ConsumptionEvidenceProvenanceV1::ReviewedCharacterization {
                    characterization_id: "characterization-1".into(),
                    characterization_record_sha256: digest('4'),
                    freshness_bucket: "freshness-1".into(),
                    freshness_observation_sha256: digest('5'),
                    terminal_at_ms: 12,
                    reviewed_at_ms: 13,
                    reviewer: "operator".into(),
                },
                true,
                '6',
                12,
            ))
            .unwrap();

        let advisory = ordinary_ids(&input, "request-2", "window-2", "attempt-2");
        assert!(matches!(
            state
                .reserve_or_reuse(
                    &advisory,
                    advisory.admission_attempt.input.authority.clone(),
                    20,
                )
                .unwrap(),
            EquivalentWorkDecisionV1::Reused(_)
        ));
        let mut manual_state = state.clone();

        let characterization_again = derive(
            input.clone(),
            one_shot_authority("entry-2"),
            characterization_trigger("characterize-2", "window-2", "attempt-2"),
            EvidencePurposeV1::Characterization,
            "freshness-1",
        );
        assert!(matches!(
            state
                .reserve_or_reuse(
                    &characterization_again,
                    characterization_again
                        .admission_attempt
                        .input
                        .authority
                        .clone(),
                    21,
                )
                .unwrap(),
            EquivalentWorkDecisionV1::Reserved(_)
        ));

        let manual = derive(
            input,
            manual_authority("manual-1"),
            manual_trigger("manual-1", "manual-attempt-1"),
            EvidencePurposeV1::ManualDiagnostic,
            "freshness-1",
        );
        assert!(matches!(
            manual_state
                .reserve_or_reuse(
                    &manual,
                    manual.admission_attempt.input.authority.clone(),
                    22,
                )
                .unwrap(),
            EquivalentWorkDecisionV1::Reserved(_)
        ));
    }

    #[test]
    fn mismatched_effective_identity_is_nonreusable_and_failed_mutation_rolls_back() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let first = ordinary_ids(&input, "request-1", "window-1", "attempt-1");
        let mut state = EquivalentWorkStateV1::new();
        let reservation = reserved(
            state
                .reserve_or_reuse(&first, first.admission_attempt.input.authority.clone(), 10)
                .unwrap(),
        );
        let bad = CompletedEquivalentEvidenceV1 {
            observed_effective_identity: effective_identity("unexpected-model"),
            ..completed(
                &reservation,
                EvidencePurposeV1::ProviderPathAdvisory,
                ConsumptionEvidenceProvenanceV1::Ordinary,
                true,
                '7',
                11,
            )
        };
        let before = state.clone();
        assert!(state.record_completed(bad.clone()).is_err());
        assert_eq!(state, before);

        state
            .record_completed(CompletedEquivalentEvidenceV1 {
                reusable: false,
                ..bad
            })
            .unwrap();
        let later = ordinary_ids(&input, "request-2", "window-2", "attempt-2");
        assert!(matches!(
            state
                .reserve_or_reuse(&later, later.admission_attempt.input.authority.clone(), 20,)
                .unwrap(),
            EquivalentWorkDecisionV1::Reserved(_)
        ));
    }

    #[test]
    fn concurrent_equivalent_sources_create_exactly_one_live_reservation() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let identities = [
            ordinary_ids(&input, "request-1", "window-1", "attempt-1"),
            ordinary_ids(&input, "request-2", "window-1", "attempt-2"),
        ];
        let state = Arc::new(Mutex::new(EquivalentWorkStateV1::new()));
        let barrier = Arc::new(Barrier::new(3));
        let handles = identities
            .into_iter()
            .map(|identities| {
                let state = Arc::clone(&state);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let authority = identities.admission_attempt.input.authority.clone();
                    state
                        .lock()
                        .unwrap()
                        .reserve_or_reuse(&identities, authority, 10)
                        .is_ok()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let successes = handles
            .into_iter()
            .map(|handle| usize::from(handle.join().unwrap()))
            .sum::<usize>();
        assert_eq!(successes, 1);
        let state = state.lock().unwrap();
        assert_eq!(state.reservations.len(), 1);
        assert_eq!(state.live_by_execution.len(), 1);
        state.validate().unwrap();
    }

    #[test]
    fn characterization_promotes_only_matching_terminal_identity_and_is_append_only() {
        let profile = fingerprint('2');
        let identity = effective_identity("gpt-5.6-luna");
        let record = CharacterizationRecordV1 {
            schema_version: 1,
            characterization_id: "characterization-1".into(),
            source: ProfileSourceRefV1 {
                kind: ProfileSourceKindV1::ScheduledAdvisory,
                schema_version: 1,
                source_sha256: digest('1'),
                row_id: "case-1".into(),
                row_sha256: digest('2'),
            },
            profile_policy_bundle_sha256: digest('3'),
            characterization_profile: profile.clone(),
            case_execution: fingerprint('4'),
            admission_attempt: fingerprint('5'),
            authority: one_shot_authority("entry-1"),
            expected_effective_identity: identity.clone(),
            observed_effective_identity: identity,
            outcome: CharacterizationOutcomeV1::CharacterizedGreen,
            evidence_sha256: digest('6'),
            terminal_at_ms: 10,
        };
        let mut state = CharacterizationStateV1::new();
        let record_sha256 = state.record_terminal(record.clone()).unwrap();
        assert_eq!(
            state.current_by_profile.get(&profile.sha256),
            Some(&record.characterization_id)
        );
        assert_eq!(
            state.record_terminal(record.clone()).unwrap(),
            record_sha256
        );

        let before = state.clone();
        let rebound = CharacterizationRecordV1 {
            evidence_sha256: digest('7'),
            ..record.clone()
        };
        assert!(state.record_terminal(rebound).is_err());
        assert_eq!(state, before);

        let duplicate_profile = CharacterizationRecordV1 {
            characterization_id: "characterization-duplicate-profile".into(),
            case_execution: fingerprint('c'),
            admission_attempt: fingerprint('d'),
            authority: one_shot_authority("entry-2"),
            evidence_sha256: digest('e'),
            ..record.clone()
        };
        assert!(state.record_terminal(duplicate_profile).is_err());
        assert_eq!(state, before);

        let mismatch_profile = fingerprint('8');
        let inconclusive = CharacterizationRecordV1 {
            characterization_id: "characterization-2".into(),
            characterization_profile: mismatch_profile.clone(),
            case_execution: fingerprint('9'),
            admission_attempt: fingerprint('a'),
            authority: one_shot_authority("entry-2"),
            observed_effective_identity: effective_identity("unexpected-model"),
            outcome: CharacterizationOutcomeV1::CharacterizationInconclusive,
            evidence_sha256: digest('b'),
            ..record
        };
        state.record_terminal(inconclusive).unwrap();
        assert!(!state
            .current_by_profile
            .contains_key(&mismatch_profile.sha256));
        state.validate().unwrap();
    }

    #[test]
    fn hold_and_quarantine_require_explicit_valid_clearance_and_expiry_fails_closed() {
        let profile = fingerprint('2');
        let execution = fingerprint('3');
        let mut state = ControlStateV1::new();
        let hold = state
            .open_safety_hold(
                profile.clone(),
                execution.clone(),
                HoldReasonV1::CleanupFailed,
                10,
            )
            .unwrap();
        assert_eq!(
            state.active_hold_by_execution.get(&execution.sha256),
            Some(&hold.hold_id)
        );
        let before = state.clone();
        assert!(state
            .clear_safety_hold(
                &execution,
                "clear-1".into(),
                9,
                "operator".into(),
                "too early".into(),
            )
            .is_err());
        assert_eq!(state, before);
        state
            .clear_safety_hold(
                &execution,
                "clear-1".into(),
                11,
                "operator".into(),
                "ownership reconciled".into(),
            )
            .unwrap();
        assert!(!state
            .active_hold_by_execution
            .contains_key(&execution.sha256));

        state
            .open_quarantine(
                profile.clone(),
                "operator".into(),
                "owner review".into(),
                20,
                21,
            )
            .unwrap();
        assert!(state.is_quarantined(&profile));
        let before = state.clone();
        assert!(state
            .close_quarantine(&profile, "operator".into(), "invalid".into(), 19)
            .is_err());
        assert_eq!(state, before);
        assert!(state.is_quarantined(&profile), "expiry never auto-resumes");
        state
            .close_quarantine(&profile, "operator".into(), "owner cleared".into(), 22)
            .unwrap();
        assert!(!state.is_quarantined(&profile));
        state.validate().unwrap();
    }

    #[test]
    fn immutable_transient_confirmation_recovery_and_unknown_paths_are_disjoint() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let immutable_ids = ordinary_ids(&input, "immutable", "window-1", "attempt-1");
        let mut immutable = ControlStateV1::new();
        let disposition = immutable
            .observe_failure(
                &immutable_ids,
                failure(
                    &immutable_ids,
                    FailureKindV1::TypedImmutable,
                    "model.removed",
                    '3',
                    10,
                ),
            )
            .unwrap();
        assert_eq!(disposition.action, FailureActionV1::Suppressed);
        assert!(immutable.confirmations.is_empty());

        let first_ids = ordinary_ids(&input, "transient", "window-1", "attempt-1");
        let mut transient = ControlStateV1::new();
        let first = transient
            .observe_failure(
                &first_ids,
                failure(
                    &first_ids,
                    FailureKindV1::TypedTransient,
                    "provider.timeout",
                    '4',
                    20,
                ),
            )
            .unwrap();
        assert_eq!(first.action, FailureActionV1::ConfirmationDue);
        let before = transient.clone();
        assert!(transient
            .authorize_confirmation(
                &first_ids,
                "window-1".into(),
                "window-2".into(),
                "repeat-1".into(),
                0,
                21,
            )
            .is_err());
        assert_eq!(transient, before);
        let authorization = transient
            .authorize_confirmation(
                &first_ids,
                "window-1".into(),
                "window-2".into(),
                "repeat-1".into(),
                1,
                21,
            )
            .unwrap();
        let before_duplicate_authorization = transient.clone();
        assert!(transient
            .authorize_confirmation(
                &first_ids,
                "window-1".into(),
                "window-2".into(),
                "repeat-duplicate".into(),
                1,
                21,
            )
            .is_err());
        assert_eq!(transient, before_duplicate_authorization);
        let confirmation_ids = derive(
            input.clone(),
            standing_authority(1),
            daily_trigger(
                "transient-confirmation",
                "window-2",
                "attempt-2",
                OptionalStableIdV1::StableId {
                    value: "repeat-1".into(),
                },
            ),
            EvidencePurposeV1::ProviderPathAdvisory,
            "freshness-1",
        );
        let wrong_authority_ids = derive(
            input.clone(),
            standing_authority(2),
            daily_trigger(
                "transient-confirmation",
                "window-2",
                "attempt-2",
                OptionalStableIdV1::StableId {
                    value: "repeat-1".into(),
                },
            ),
            EvidencePurposeV1::ProviderPathAdvisory,
            "freshness-1",
        );
        let before_wrong_authority = transient.clone();
        assert!(transient
            .observe_confirmation_failure(
                &authorization.confirmation_id,
                &wrong_authority_ids,
                failure(
                    &wrong_authority_ids,
                    FailureKindV1::TypedTransient,
                    "provider.timeout",
                    '5',
                    22,
                ),
            )
            .is_err());
        assert_eq!(transient, before_wrong_authority);
        let second = transient
            .observe_confirmation_failure(
                &authorization.confirmation_id,
                &confirmation_ids,
                failure(
                    &confirmation_ids,
                    FailureKindV1::TypedTransient,
                    "provider.timeout",
                    '5',
                    22,
                ),
            )
            .unwrap();
        assert_eq!(second.action, FailureActionV1::Suppressed);
        assert_eq!(second.identical_complete_occurrences, 2);

        let mut recovered = ControlStateV1::new();
        recovered
            .observe_failure(
                &first_ids,
                failure(
                    &first_ids,
                    FailureKindV1::TypedTransient,
                    "provider.timeout",
                    '6',
                    30,
                ),
            )
            .unwrap();
        let authorization = recovered
            .authorize_confirmation(
                &first_ids,
                "window-1".into(),
                "window-2".into(),
                "repeat-2".into(),
                1,
                31,
            )
            .unwrap();
        let recovered_ids = derive(
            input.clone(),
            standing_authority(1),
            daily_trigger(
                "transient-recovery",
                "window-2",
                "attempt-3",
                OptionalStableIdV1::StableId {
                    value: "repeat-2".into(),
                },
            ),
            EvidencePurposeV1::ProviderPathAdvisory,
            "freshness-1",
        );
        let disposition = recovered
            .observe_confirmation_success(
                &authorization.confirmation_id,
                &recovered_ids,
                digest('7'),
                32,
            )
            .unwrap();
        assert_eq!(disposition.action, FailureActionV1::Recovered);

        let unknown_first = ordinary_ids(&input, "unknown-1", "window-3", "attempt-4");
        let unknown_second = ordinary_ids(&input, "unknown-2", "window-4", "attempt-5");
        let mut unknown = ControlStateV1::new();
        unknown
            .observe_failure(
                &unknown_first,
                failure(
                    &unknown_first,
                    FailureKindV1::CandidateUnknown,
                    "catalog.unavailable",
                    '8',
                    40,
                ),
            )
            .unwrap();
        let disposition = unknown
            .observe_failure(
                &unknown_second,
                failure(
                    &unknown_second,
                    FailureKindV1::CandidateUnknown,
                    "catalog.unavailable",
                    '9',
                    41,
                ),
            )
            .unwrap();
        assert_eq!(disposition.action, FailureActionV1::UnknownRetained);
        assert_eq!(disposition.identical_complete_occurrences, 2);
        assert!(unknown.confirmations.is_empty());
    }

    #[test]
    fn unauthorized_repeat_and_untyped_confirmation_never_suppress() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let first_ids = ordinary_ids(&input, "untyped", "window-1", "attempt-1");
        let mut state = ControlStateV1::new();
        state
            .observe_failure(
                &first_ids,
                failure(
                    &first_ids,
                    FailureKindV1::UntypedTransient,
                    "untyped.failure",
                    '3',
                    10,
                ),
            )
            .unwrap();
        let repeat_ids = ordinary_ids(&input, "untyped-repeat", "window-2", "attempt-2");
        let before = state.clone();
        assert!(state
            .observe_failure(
                &repeat_ids,
                failure(
                    &repeat_ids,
                    FailureKindV1::UntypedTransient,
                    "untyped.failure",
                    '4',
                    11,
                ),
            )
            .is_err());
        assert_eq!(state, before);

        let different_repeat = ordinary_ids(&input, "different-repeat", "window-2", "attempt-x");
        assert!(state
            .observe_failure(
                &different_repeat,
                failure(
                    &different_repeat,
                    FailureKindV1::TypedImmutable,
                    "different.immutable",
                    '4',
                    11,
                ),
            )
            .is_err());
        assert_eq!(state, before);

        let authorization = state
            .authorize_confirmation(
                &first_ids,
                "window-1".into(),
                "window-2".into(),
                "repeat-1".into(),
                1,
                12,
            )
            .unwrap();
        let confirmation_ids = derive(
            input,
            standing_authority(1),
            daily_trigger(
                "untyped-confirmation",
                "window-2",
                "attempt-3",
                OptionalStableIdV1::StableId {
                    value: "repeat-1".into(),
                },
            ),
            EvidencePurposeV1::ProviderPathAdvisory,
            "freshness-1",
        );
        let disposition = state
            .observe_confirmation_failure(
                &authorization.confirmation_id,
                &confirmation_ids,
                failure(
                    &confirmation_ids,
                    FailureKindV1::UntypedTransient,
                    "untyped.failure",
                    '5',
                    13,
                ),
            )
            .unwrap();
        assert_eq!(disposition.action, FailureActionV1::UnknownRetained);
        assert_ne!(disposition.action, FailureActionV1::Suppressed);
    }

    #[test]
    fn authorized_nonidentical_transient_confirmation_becomes_unknown_not_new_due() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let first_ids = ordinary_ids(&input, "request-1", "window-1", "attempt-1");
        let mut state = ControlStateV1::new();
        state
            .observe_failure(
                &first_ids,
                failure(
                    &first_ids,
                    FailureKindV1::TypedTransient,
                    "provider.timeout",
                    '3',
                    10,
                ),
            )
            .unwrap();
        let authorization = state
            .authorize_confirmation(
                &first_ids,
                "window-1".into(),
                "window-2".into(),
                "repeat-1".into(),
                1,
                11,
            )
            .unwrap();
        let confirmation_ids = derive(
            input,
            standing_authority(1),
            daily_trigger(
                "request-2",
                "window-2",
                "attempt-2",
                OptionalStableIdV1::StableId {
                    value: "repeat-1".into(),
                },
            ),
            EvidencePurposeV1::ProviderPathAdvisory,
            "freshness-1",
        );
        let disposition = state
            .observe_confirmation_failure(
                &authorization.confirmation_id,
                &confirmation_ids,
                failure(
                    &confirmation_ids,
                    FailureKindV1::TypedTransient,
                    "provider.connection-reset",
                    '4',
                    12,
                ),
            )
            .unwrap();
        assert_eq!(disposition.failure_kind, FailureKindV1::CandidateUnknown);
        assert_eq!(disposition.action, FailureActionV1::UnknownRetained);
        assert_eq!(disposition.identical_complete_occurrences, 1);
        assert!(state.active_confirmation_by_execution.is_empty());
    }

    #[test]
    fn observed_identity_mismatch_retains_admitted_keys_and_atomically_opens_hold() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let identities = ordinary_ids(&input, "request-1", "window-1", "attempt-1");
        let before_identities = identities.clone();
        let mut state = ControlStateV1::new();
        let (disposition, hold) = state
            .record_identity_mismatch(
                &identities,
                &effective_identity("unexpected-model"),
                digest('3'),
                10,
            )
            .unwrap();
        assert_eq!(identities, before_identities);
        assert_eq!(disposition.action, FailureActionV1::UnknownRetained);
        assert_eq!(hold.reason, HoldReasonV1::IdentityDriftAfterEffect);
        assert_eq!(hold.case_execution, identities.case_execution.fingerprint);
        assert_eq!(
            identities.equivalent_work_key,
            before_identities.equivalent_work_key
        );
        assert!(state
            .active_hold_by_execution
            .contains_key(&identities.case_execution.fingerprint.sha256));

        let before = state.clone();
        assert!(state
            .record_identity_mismatch(
                &identities,
                &input.expected_effective_identity,
                digest('4'),
                11,
            )
            .is_err());
        assert_eq!(state, before);
    }

    #[test]
    fn quarantine_and_other_pre_effect_blocks_do_not_enter_waste_state() {
        let profile = fingerprint('2');
        let mut state = ControlStateV1::new();
        state
            .open_quarantine(
                profile.clone(),
                "operator".into(),
                "manual block".into(),
                10,
                20,
            )
            .unwrap();
        assert!(state.is_quarantined(&profile));
        assert!(state.failures.is_empty());
        assert!(state.confirmations.is_empty());
        assert!(state.active_hold_by_execution.is_empty());

        let serialized_before = serde_json::to_vec(&state).unwrap();
        for _external_block in ["authority", "budget", "quarantine"] {
            assert_eq!(serde_json::to_vec(&state).unwrap(), serialized_before);
        }
    }

    #[test]
    fn materialized_control_and_equivalent_indexes_fail_closed_when_orphaned() {
        let profile = fingerprint('2');
        let input = execution_input(&profile);
        let identities = ordinary_ids(&input, "request-1", "window-1", "attempt-1");
        let mut equivalent = EquivalentWorkStateV1::new();
        equivalent
            .reserve_or_reuse(
                &identities,
                identities.admission_attempt.input.authority.clone(),
                10,
            )
            .unwrap();
        equivalent.live_by_execution.clear();
        assert!(equivalent.validate().is_err());

        let mut controls = ControlStateV1::new();
        controls
            .open_safety_hold(
                profile,
                identities.case_execution.fingerprint.clone(),
                HoldReasonV1::WorkerFailed,
                10,
            )
            .unwrap();
        controls.active_hold_by_execution.clear();
        assert!(controls.validate().is_err());
    }

    #[test]
    fn manual_and_characterization_consumptions_are_rejected_by_schema() {
        let base = ConsumptionRecordV1 {
            schema_version: 1,
            consumption_id: "consumption-1".into(),
            equivalent_work_key: digest('1'),
            evidence_sha256: digest('2'),
            requested_purpose: EvidencePurposeV1::ManualDiagnostic,
            satisfied_purpose: EvidencePurposeV1::ManualDiagnostic,
            provenance: ConsumptionEvidenceProvenanceV1::Ordinary,
            characterization_profile: fingerprint('3'),
            case_execution: fingerprint('4'),
            admission_attempt: fingerprint('5'),
            authority: manual_authority("manual-1"),
            consumed_at_ms: 10,
        };
        assert!(base.validate().is_err());
        assert!(ConsumptionRecordV1 {
            requested_purpose: EvidencePurposeV1::Characterization,
            satisfied_purpose: EvidencePurposeV1::Characterization,
            authority: one_shot_authority("entry-1"),
            ..base
        }
        .validate()
        .is_err());
    }
}
