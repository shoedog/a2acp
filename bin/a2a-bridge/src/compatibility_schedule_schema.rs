//! Strict R3d0 record schemas used by later scheduling slices.
//!
//! These DTOs and validators are inert contracts. Parsing them performs no authority mutation,
//! credential access, provider call, registry/image operation, or GitHub publication.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::compatibility_schedule::{
    EffectCapsV1, EffectClassV1, EvidencePurposeV1, ReplicationModeV1, TriggerKindV1,
};
use crate::{compatibility, local_file, BoxError};

const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_ITEMS: usize = 256;
const MAX_CANDIDATE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FingerprintV1 {
    pub(super) schema_version: u16,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EffectiveIdentityV1 {
    pub(super) model: String,
    #[serde(default)]
    pub(super) effort: Option<String>,
    #[serde(default)]
    pub(super) mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalSha256V1 {
    Absent,
    Sha256 { value: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalStableIdV1 {
    Absent,
    StableId { value: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ExactExecutionTargetV1 {
    RepositorySnapshot {
        repository: String,
        head_sha256: String,
        tree_sha256: String,
        range_start_exclusive: OptionalSha256V1,
    },
    TestMerge {
        repository: String,
        pull_request: u64,
        base_sha256: String,
        head_sha256: String,
        merge_sha256: String,
        merge_ref: String,
        tree_sha256: String,
        ordered_parents: Vec<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CandidateBinaryIdentityV1 {
    pub(super) sha256: String,
    pub(super) length_bytes: u64,
    pub(super) build_provenance_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ExactExecutionBindingsV1 {
    pub(super) source_sha256: String,
    pub(super) row_sha256: String,
    pub(super) run_manifest_sha256: String,
    pub(super) generated_config_sha256: String,
    pub(super) pin_set_sha256: String,
    pub(super) resolution_bundle: OptionalSha256V1,
    pub(super) package_integrity_sha256: String,
    pub(super) image_digest: OptionalSha256V1,
    pub(super) base_image_digest: OptionalSha256V1,
    pub(super) environment_sha256: String,
    pub(super) prerequisites_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CaseExecutionFingerprintInputV1 {
    pub(super) schema_version: u16,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) target: ExactExecutionTargetV1,
    pub(super) candidate: CandidateBinaryIdentityV1,
    pub(super) bindings: ExactExecutionBindingsV1,
    pub(super) requested_identity: EffectiveIdentityV1,
    pub(super) expected_effective_identity: EffectiveIdentityV1,
    pub(super) actual_caps: EffectCapsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CaseExecutionFingerprintRecordV1 {
    pub(super) schema_version: u16,
    pub(super) input: CaseExecutionFingerprintInputV1,
    pub(super) fingerprint: FingerprintV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum TriggerSourceV1 {
    ManualCharacterizationCli,
    ManualCompatibilityCli,
    DailyLaunchd,
    ScheduledMainCoalescer,
    TestMergeWatcher,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionTriggerIdentityV1 {
    pub(super) source: TriggerSourceV1,
    pub(super) kind: TriggerKindV1,
    pub(super) request_id: String,
    pub(super) window_id: String,
    pub(super) attempt_id: String,
    pub(super) repeat_nonce: OptionalStableIdV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionAttemptFingerprintInputV1 {
    pub(super) schema_version: u16,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) trigger: AdmissionTriggerIdentityV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionAttemptFingerprintRecordV1 {
    pub(super) schema_version: u16,
    pub(super) input: AdmissionAttemptFingerprintInputV1,
    pub(super) fingerprint: FingerprintV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProfileSourceKindV1 {
    ScheduledAdvisory,
    ClaimedSupportGate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProfileSourceRefV1 {
    pub(super) kind: ProfileSourceKindV1,
    pub(super) schema_version: u16,
    pub(super) source_sha256: String,
    pub(super) row_id: String,
    pub(super) row_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CharacterizationOnceAuthorityV1 {
    pub(super) batch_authorization_id: String,
    pub(super) batch_authorization_sha256: String,
    pub(super) entry_id: String,
    pub(super) generation: u64,
    pub(super) entry_sha256: String,
    pub(super) consumption_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct StandingGrantAuthorityV1 {
    pub(super) grant_id: String,
    pub(super) generation: u64,
    pub(super) grant_sha256: String,
    pub(super) characterization_id: String,
    pub(super) characterization_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ManualAcknowledgementAuthorityV1 {
    pub(super) manual_admission_sha256: String,
    pub(super) request_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AdmissionAuthorityV1 {
    CharacterizationOnce(CharacterizationOnceAuthorityV1),
    StandingGrant(StandingGrantAuthorityV1),
    ManualAcknowledgement(ManualAcknowledgementAuthorityV1),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct OneShotCharacterizationEntryV1 {
    pub(super) entry_id: String,
    pub(super) generation: u64,
    pub(super) entry_sha256: String,
    pub(super) consumption_nonce: String,
    pub(super) source: ProfileSourceRefV1,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) characterization_execution: FingerprintV1,
    pub(super) proposed_effective_identity: EffectiveIdentityV1,
    pub(super) provider_family: String,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) caps: EffectCapsV1,
    pub(super) command: String,
    pub(super) not_before_ms: i64,
    pub(super) expires_at_ms: i64,
    pub(super) revocation_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CharacterizationAuthorizationV1 {
    pub(super) schema_version: u16,
    pub(super) authorization_id: String,
    pub(super) authorization_sha256: String,
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) host_identity_sha256: String,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) scheduler_binary_sha256: String,
    pub(super) price_snapshot_sha256: String,
    pub(super) legacy_inventory_sha256: String,
    pub(super) issued_at_ms: i64,
    #[serde(default)]
    pub(super) entries: Vec<OneShotCharacterizationEntryV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CharacterizedGrantProfileV1 {
    pub(super) case_id: String,
    pub(super) provider_family: String,
    pub(super) source: ProfileSourceRefV1,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) characterization_id: String,
    pub(super) characterization_sha256: String,
    pub(super) effective_identity: EffectiveIdentityV1,
    pub(super) caps: EffectCapsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AggregateBudgetCapsV1 {
    pub(super) max_attempts: u64,
    pub(super) max_tokens: u64,
    pub(super) max_cost_microusd: u64,
    pub(super) max_time_secs: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NamedBudgetCapsV1 {
    pub(super) id: String,
    pub(super) caps: AggregateBudgetCapsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TriggerBudgetCapsV1 {
    pub(super) trigger: TriggerKindV1,
    pub(super) caps: AggregateBudgetCapsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GrantBudgetPolicyV1 {
    pub(super) per_case: Vec<NamedBudgetCapsV1>,
    pub(super) per_trigger_pool: Vec<TriggerBudgetCapsV1>,
    pub(super) per_provider: Vec<NamedBudgetCapsV1>,
    pub(super) utc_day: AggregateBudgetCapsV1,
    pub(super) rolling_24h: AggregateBudgetCapsV1,
    pub(super) protected_scheduled: AggregateBudgetCapsV1,
    pub(super) protected_test_merge: AggregateBudgetCapsV1,
    pub(super) manual_unallocated: AggregateBudgetCapsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LaunchdBindingV1 {
    pub(super) label: String,
    pub(super) plist_sha256: String,
    pub(super) trigger: TriggerKindV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderEffectGrantV1 {
    pub(super) schema_version: u16,
    pub(super) grant_id: String,
    pub(super) generation: u64,
    pub(super) grant_sha256: String,
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) host_identity_sha256: String,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) scheduler_binary_sha256: String,
    pub(super) price_snapshot_sha256: String,
    pub(super) price_snapshot_observed_at_ms: i64,
    pub(super) price_snapshot_valid_until_ms: i64,
    pub(super) legacy_inventory_sha256: String,
    pub(super) triggers: Vec<TriggerKindV1>,
    pub(super) case_ids: Vec<String>,
    pub(super) provider_families: Vec<String>,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) per_run_caps: EffectCapsV1,
    pub(super) budgets: GrantBudgetPolicyV1,
    pub(super) confirmation_allowance: u8,
    pub(super) launchd: Vec<LaunchdBindingV1>,
    pub(super) profiles: Vec<CharacterizedGrantProfileV1>,
    pub(super) not_before_ms: i64,
    pub(super) expires_at_ms: i64,
    pub(super) revocation_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ManualAdmissionV1 {
    pub(super) schema_version: u16,
    pub(super) request_nonce: String,
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) scheduler_binary_sha256: String,
    pub(super) input_source_sha256: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) evidence_purpose: EvidencePurposeV1,
    pub(super) freshness_bucket: String,
    pub(super) command: String,
    pub(super) caps: EffectCapsV1,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) retry_cap: u8,
    pub(super) fallback_cap: u8,
    pub(super) acknowledged_billable: bool,
    pub(super) issued_at_ms: i64,
    pub(super) expires_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct StorageConsentV1 {
    pub(super) schema_version: u16,
    pub(super) consent_id: String,
    pub(super) consent_sha256: String,
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) evidence_classes: Vec<EvidenceClassV1>,
    pub(super) cold_root: String,
    pub(super) replication_mode: ReplicationModeV1,
    pub(super) file_provider_domain_id: String,
    pub(super) not_before_ms: i64,
    pub(super) expires_at_ms: i64,
    pub(super) revocation_generation: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum CharacterizationOutcomeV1 {
    CharacterizedGreen,
    CharacterizedKnownIssue,
    CharacterizationInconclusive,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CharacterizationRecordV1 {
    pub(super) schema_version: u16,
    pub(super) characterization_id: String,
    pub(super) source: ProfileSourceRefV1,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) expected_effective_identity: EffectiveIdentityV1,
    pub(super) observed_effective_identity: EffectiveIdentityV1,
    pub(super) outcome: CharacterizationOutcomeV1,
    pub(super) evidence_sha256: String,
    pub(super) terminal_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum HoldReasonV1 {
    PromptAcceptanceUnknown,
    CleanupFailed,
    ProcessExitUnproved,
    ArtifactFailed,
    LedgerUnreconciled,
    IdentityDriftAfterEffect,
    DuplicateBillableProcess,
    WorkerFailed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SafetyHoldV1 {
    pub(super) schema_version: u16,
    pub(super) hold_id: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) reason: HoldReasonV1,
    pub(super) created_at_ms: i64,
    #[serde(default)]
    pub(super) cleared_at_ms: Option<i64>,
    #[serde(default)]
    pub(super) clearance_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct QuarantineV1 {
    pub(super) schema_version: u16,
    pub(super) quarantine_id: String,
    pub(super) profile: FingerprintV1,
    pub(super) operator: String,
    pub(super) reason: String,
    pub(super) created_at_ms: i64,
    pub(super) expires_at_ms: i64,
    pub(super) active: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum FailureKindV1 {
    TypedImmutable,
    TypedTransient,
    UntypedTransient,
    CandidateUnknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum FailureActionV1 {
    Suppressed,
    ConfirmationDue,
    UnknownRetained,
    Recovered,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FailureDispositionV1 {
    pub(super) schema_version: u16,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) evidence_sha256: String,
    pub(super) failure_kind: FailureKindV1,
    pub(super) typed_code: String,
    pub(super) identical_complete_occurrences: u8,
    pub(super) action: FailureActionV1,
    pub(super) first_seen_ms: i64,
    pub(super) last_seen_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum ImpactClassV1 {
    AcpRuntime,
    ContainerRuntime,
    ModelCapability,
    Authentication,
    CompatibilityCore,
    TestsOnly,
    DocumentationOnly,
    NewProvider,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TestMergeIdentityV1 {
    pub(super) repository: String,
    pub(super) pull_request: u64,
    pub(super) base_sha256: String,
    pub(super) head_sha256: String,
    pub(super) merge_sha256: String,
    pub(super) merge_ref: String,
    pub(super) tree_sha256: String,
    pub(super) ordered_parents: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ImpactRecordV1 {
    pub(super) schema_version: u16,
    pub(super) classifier_sha256: String,
    pub(super) target: TestMergeIdentityV1,
    pub(super) classes: Vec<ImpactClassV1>,
    pub(super) due_case_ids: Vec<String>,
    pub(super) no_impact_proved: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AccountingClassV1 {
    Characterization,
    Scheduled,
    TestMerge,
    Manual,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum LedgerDispositionV1 {
    ReleasedPreEffect,
    ChargedTerminal,
    ChargedConservative,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct UsageChargeV1 {
    pub(super) attempts: u8,
    pub(super) tokens: u64,
    pub(super) cost_microusd: u64,
    pub(super) elapsed_millis: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LedgerReservationV1 {
    pub(super) schema_version: u16,
    pub(super) reservation_id: String,
    pub(super) attempt_idempotency_key: String,
    pub(super) accounting_class: AccountingClassV1,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) equivalent_work_key: String,
    pub(super) evidence_purpose: EvidencePurposeV1,
    pub(super) freshness_bucket: String,
    pub(super) repeat_nonce: OptionalStableIdV1,
    pub(super) caps: EffectCapsV1,
    pub(super) utc_day_id: String,
    pub(super) rolling_window_id: String,
    pub(super) reserved_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LedgerReconciliationV1 {
    pub(super) schema_version: u16,
    pub(super) reservation_id: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) equivalent_work_key: String,
    pub(super) terminal_evidence_sha256: String,
    pub(super) disposition: LedgerDispositionV1,
    pub(super) charged_usage: UsageChargeV1,
    pub(super) prompt_may_have_been_accepted: bool,
    pub(super) reconciled_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "record_kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum LedgerRecordV1 {
    Reservation(LedgerReservationV1),
    Reconciliation(LedgerReconciliationV1),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EquivalentWorkReservationV1 {
    pub(super) schema_version: u16,
    pub(super) reservation_id: String,
    pub(super) equivalent_work_key: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) evidence_purpose: EvidencePurposeV1,
    pub(super) freshness_bucket: String,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) reserved_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ConsumptionRecordV1 {
    pub(super) schema_version: u16,
    pub(super) consumption_id: String,
    pub(super) equivalent_work_key: String,
    pub(super) evidence_sha256: String,
    pub(super) requested_purpose: EvidencePurposeV1,
    pub(super) satisfied_purpose: EvidencePurposeV1,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) consumed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ScheduledExecutionSourceV1 {
    pub(super) schema_version: u16,
    pub(super) source_sha256: String,
    pub(super) source: ProfileSourceRefV1,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: CaseExecutionFingerprintRecordV1,
    pub(super) admission_attempt: AdmissionAttemptFingerprintRecordV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) trigger: TriggerKindV1,
    pub(super) requested_identity: EffectiveIdentityV1,
    pub(super) expected_effective_identity: EffectiveIdentityV1,
    pub(super) caps: EffectCapsV1,
    pub(super) retry_cap: u8,
    pub(super) fallback_cap: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ClaimedSupportCharacterizationSourceV1 {
    pub(super) schema_version: u16,
    pub(super) source_sha256: String,
    pub(super) source: ProfileSourceRefV1,
    pub(super) production_manifest_sha256: String,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) characterization_execution: CaseExecutionFingerprintRecordV1,
    pub(super) admission_attempt: AdmissionAttemptFingerprintRecordV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) trigger: TriggerKindV1,
    pub(super) pinned_config_sha256: String,
    pub(super) requested_identity: EffectiveIdentityV1,
    pub(super) expected_effective_identity: EffectiveIdentityV1,
    pub(super) caps: EffectCapsV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalRecordRefV1 {
    Absent,
    Record { id: String, sha256: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalEffectiveIdentityV1 {
    Absent,
    Identity {
        model: String,
        #[serde(default)]
        effort: Option<String>,
        #[serde(default)]
        mode: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum CheckBindingV1 {
    Absent,
    TestMergeResult {
        target: Box<TestMergeIdentityV1>,
        guarded_observation_sha256: String,
        classifier_sha256: String,
        required_rule_sha256: String,
        context: String,
        expected_source_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ScheduleEvidenceRecordV1 {
    pub(super) schema_version: u16,
    pub(super) schedule_record_id: String,
    pub(super) trigger: TriggerKindV1,
    pub(super) source: ProfileSourceRefV1,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) aggregate: OptionalSha256V1,
    pub(super) evidence_index_id: String,
    pub(super) check: CheckBindingV1,
    pub(super) storage_consent: OptionalRecordRefV1,
    pub(super) quarantine: OptionalRecordRefV1,
    pub(super) characterization: OptionalRecordRefV1,
    pub(super) window_id: String,
    pub(super) attempt_idempotency_key: String,
    pub(super) equivalent_work_key: String,
    pub(super) consumption: OptionalRecordRefV1,
    pub(super) repeat_nonce: OptionalStableIdV1,
    pub(super) ledger_reservation_id: String,
    pub(super) budget_reservation_sha256: String,
    pub(super) ledger_reconciliation: OptionalSha256V1,
    pub(super) deadline_derivation_sha256: String,
    pub(super) preflight_results_sha256: String,
    pub(super) admission_lock_holder_sha256: String,
    pub(super) supervisor_record_sha256: String,
    pub(super) freshness_observation_sha256: String,
    pub(super) requested_identity: EffectiveIdentityV1,
    pub(super) expected_effective_identity: EffectiveIdentityV1,
    pub(super) observed_effective_identity: OptionalEffectiveIdentityV1,
    pub(super) publication_outbox: OptionalRecordRefV1,
    pub(super) status_publication: OptionalSha256V1,
    pub(super) affected_case_ids: Vec<String>,
    pub(super) created_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PublicationOutboxStateV1 {
    CreateIntent,
    CreateUnknown,
    RemotePending,
    Prepared,
    UpdateUnknown,
    RemotelyObserved,
    Confirmed,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum CheckConclusionV1 {
    Success,
    Failure,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalCheckRunIdV1 {
    Absent,
    CheckRun { id: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalCheckConclusionV1 {
    Absent,
    Conclusion { value: CheckConclusionV1 },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PublicationOutboxV1 {
    pub(super) schema_version: u16,
    pub(super) outbox_id: String,
    pub(super) state: PublicationOutboxStateV1,
    pub(super) repository: String,
    pub(super) pull_request: u64,
    pub(super) test_merge_sha256: String,
    pub(super) context: String,
    pub(super) app_id: String,
    pub(super) external_id: String,
    pub(super) check_run: OptionalCheckRunIdV1,
    pub(super) terminal_consumption: OptionalStableIdV1,
    pub(super) desired_conclusion: OptionalCheckConclusionV1,
    pub(super) evidence_set: OptionalSha256V1,
    pub(super) final_guard: OptionalSha256V1,
    pub(super) remote_observation: OptionalSha256V1,
    pub(super) remote_observation_attempts: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum EvidenceClassV1 {
    RoutineGreen,
    PreflightBlocked,
    FailedOrUnknown,
    ManualCompatibility,
    Incident,
    PromotionRelease,
    AuthorizationBudgetAudit,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceIndexEntryV1 {
    pub(super) evidence_id: String,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) full_evidence_sha256: String,
    pub(super) compact_record_sha256: String,
    pub(super) hot_path: String,
    #[serde(default)]
    pub(super) cold_path: Option<String>,
    pub(super) full_retain_until_ms: i64,
    pub(super) compact_retain_until_ms: i64,
    pub(super) pinned: bool,
    pub(super) lease_count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceIndexV1 {
    pub(super) schema_version: u16,
    pub(super) index_id: String,
    pub(super) generation: u64,
    pub(super) entries: Vec<EvidenceIndexEntryV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ScheduleCaseLifecycleV1 {
    CharacterizationRequired,
    CharacterizedGreen,
    CharacterizedKnownIssue,
    CharacterizationInconclusive,
    ScheduledActive,
    RequiredGateActive,
    OperatorQuarantined,
    Deferred,
    Retired,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ScheduleCaseStatusV1 {
    pub(super) case_id: String,
    pub(super) lifecycle: ScheduleCaseLifecycleV1,
    #[serde(default)]
    pub(super) last_outcome: Option<String>,
    #[serde(default)]
    pub(super) hold_id: Option<String>,
    #[serde(default)]
    pub(super) quarantine_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ScheduleStatusV1 {
    pub(super) schema_version: u16,
    pub(super) generated_at_ms: i64,
    pub(super) policy_sha256: String,
    #[serde(default)]
    pub(super) provider_grant_sha256: Option<String>,
    #[serde(default)]
    pub(super) storage_consent_sha256: Option<String>,
    pub(super) ledger_headroom_sha256: String,
    pub(super) storage_state: String,
    pub(super) missed_ticks: u64,
    pub(super) fresh_one_shot_compatibility: String,
    pub(super) shared_operator_health: String,
    pub(super) cases: Vec<ScheduleCaseStatusV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum DogfoodTaskClassV1 {
    BoundedSummaryDocsLightBrainstorm,
    SmallSpecifiedImplementation,
    NormalImplementation,
    SpecDesignArchitectureAuthoring,
    CleanroomSpecTechnicalDesign,
    AdversarialDesignImplementationReview,
    ReleaseCompatibilityReview,
    FullBranchReview,
    RequirementsBrainstormAnalysisGrooming,
    ConcurrencyTransactionCriticalProof,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DogfoodRoutingRuleV1 {
    pub(super) task_class: DogfoodTaskClassV1,
    pub(super) primary_model_class: String,
    pub(super) effort: String,
    #[serde(default)]
    pub(super) second_opinion: Option<String>,
    pub(super) max_allowed: bool,
    pub(super) fable_allowed_only_when_hard_complex: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DogfoodRoutingPolicyV1 {
    pub(super) schema_version: u16,
    pub(super) advisory_only: bool,
    pub(super) audit_required: bool,
    pub(super) rules: Vec<DogfoodRoutingRuleV1>,
}

fn bounded_text(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_TEXT_BYTES
        || value.chars().any(char::is_control)
        || compatibility::looks_like_secret(value)
    {
        return Err(format!(
            "schedule schema: {label} must be non-empty, unpadded, control-free UTF-8 of at most {MAX_TEXT_BYTES} bytes"
        )
        .into());
    }
    Ok(())
}

fn stable_id(label: &str, value: &str) -> Result<(), BoxError> {
    bounded_text(label, value)?;
    let mut bytes = value.bytes();
    if value.len() > MAX_ID_BYTES
        || !matches!(bytes.next(), Some(b'a'..=b'z') | Some(b'0'..=b'9'))
        || !bytes.all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b':')
        })
    {
        return Err(format!("schedule schema: {label} must be a stable id").into());
    }
    Ok(())
}

fn sha256(label: &str, value: &str) -> Result<(), BoxError> {
    if !local_file::valid_sha256(value) || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(
            format!("schedule schema: {label} must be one lowercase SHA-256 digest").into(),
        );
    }
    Ok(())
}

fn fingerprint(label: &str, value: &FingerprintV1) -> Result<(), BoxError> {
    if value.schema_version != 1 {
        return Err(format!("schedule schema: {label} schema_version must be 1").into());
    }
    sha256(&format!("{label}.sha256"), &value.sha256)
}

fn optional_sha256(label: &str, value: &OptionalSha256V1) -> Result<(), BoxError> {
    if let OptionalSha256V1::Sha256 { value } = value {
        sha256(label, value)?;
    }
    Ok(())
}

fn optional_stable_id(label: &str, value: &OptionalStableIdV1) -> Result<(), BoxError> {
    if let OptionalStableIdV1::StableId { value } = value {
        stable_id(label, value)?;
    }
    Ok(())
}

fn optional_record_ref(label: &str, value: &OptionalRecordRefV1) -> Result<(), BoxError> {
    if let OptionalRecordRefV1::Record { id, sha256: value } = value {
        stable_id(&format!("{label} id"), id)?;
        sha256(label, value)?;
    }
    Ok(())
}

fn optional_effective_identity(
    label: &str,
    value: &OptionalEffectiveIdentityV1,
) -> Result<(), BoxError> {
    if let OptionalEffectiveIdentityV1::Identity {
        model,
        effort,
        mode,
    } = value
    {
        effective_identity(
            label,
            &EffectiveIdentityV1 {
                model: model.clone(),
                effort: effort.clone(),
                mode: mode.clone(),
            },
        )?;
    }
    Ok(())
}

fn effective_identity(label: &str, value: &EffectiveIdentityV1) -> Result<(), BoxError> {
    bounded_text(&format!("{label}.model"), &value.model)?;
    for (field, value) in [("effort", &value.effort), ("mode", &value.mode)] {
        if let Some(value) = value {
            bounded_text(&format!("{label}.{field}"), value)?;
        }
    }
    Ok(())
}

fn canonical_input_sha256<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("schedule schema: cannot canonicalize {label}: {error}"))?;
    Ok(local_file::sha256_hex(&bytes))
}

fn validate_execution_target(target: &ExactExecutionTargetV1) -> Result<(), BoxError> {
    match target {
        ExactExecutionTargetV1::RepositorySnapshot {
            repository,
            head_sha256,
            tree_sha256,
            range_start_exclusive,
        } => {
            bounded_text("execution repository", repository)?;
            sha256("execution head", head_sha256)?;
            sha256("execution tree", tree_sha256)?;
            optional_sha256("execution range start", range_start_exclusive)
        }
        ExactExecutionTargetV1::TestMerge {
            repository,
            pull_request,
            base_sha256,
            head_sha256,
            merge_sha256,
            merge_ref,
            tree_sha256,
            ordered_parents,
        } => {
            bounded_text("test-merge repository", repository)?;
            for (label, value) in [
                ("test-merge base", base_sha256),
                ("test-merge head", head_sha256),
                ("test-merge result", merge_sha256),
                ("test-merge tree", tree_sha256),
            ] {
                sha256(label, value)?;
            }
            if *pull_request == 0
                || merge_ref != &format!("refs/pull/{pull_request}/merge")
                || ordered_parents != &[base_sha256.clone(), head_sha256.clone()]
            {
                return Err(
                    "schedule schema: execution test-merge identity is not canonical".into(),
                );
            }
            Ok(())
        }
    }
}

fn validate_trigger_identity(trigger: &AdmissionTriggerIdentityV1) -> Result<(), BoxError> {
    for (label, value) in [
        ("trigger request id", &trigger.request_id),
        ("trigger window id", &trigger.window_id),
        ("trigger attempt id", &trigger.attempt_id),
    ] {
        stable_id(label, value)?;
    }
    optional_stable_id("trigger repeat nonce", &trigger.repeat_nonce)?;
    let matches = matches!(
        (trigger.source, trigger.kind),
        (
            TriggerSourceV1::ManualCharacterizationCli,
            TriggerKindV1::ManualCharacterization
        ) | (
            TriggerSourceV1::ManualCompatibilityCli,
            TriggerKindV1::ManualCompatibility
        ) | (TriggerSourceV1::DailyLaunchd, TriggerKindV1::Daily)
            | (
                TriggerSourceV1::ScheduledMainCoalescer,
                TriggerKindV1::ScheduledMain
            )
            | (TriggerSourceV1::TestMergeWatcher, TriggerKindV1::TestMerge)
    );
    if !matches {
        return Err("schedule schema: trigger source and kind disagree".into());
    }
    Ok(())
}

fn time_range(label: &str, start: i64, end: i64) -> Result<(), BoxError> {
    if start <= 0 || end <= start {
        return Err(format!("schedule schema: {label} requires 0 < start < end").into());
    }
    Ok(())
}

fn unique_ids<'a>(label: &str, values: impl IntoIterator<Item = &'a str>) -> Result<(), BoxError> {
    let mut seen = BTreeSet::new();
    for value in values {
        stable_id(label, value)?;
        if !seen.insert(value) {
            return Err(format!("schedule schema: duplicate {label} {value:?}").into());
        }
    }
    Ok(())
}

fn effects(label: &str, values: &[EffectClassV1]) -> Result<(), BoxError> {
    if values.is_empty()
        || values.len() > MAX_ITEMS
        || values.iter().collect::<BTreeSet<_>>().len() != values.len()
    {
        return Err(format!("schedule schema: {label} must be non-empty and unique").into());
    }
    Ok(())
}

fn aggregate_budget_caps(label: &str, caps: &AggregateBudgetCapsV1) -> Result<(), BoxError> {
    if caps.max_attempts == 0
        || caps.max_attempts > 10_000
        || caps.max_tokens == 0
        || caps.max_tokens > 1_000_000_000
        || caps.max_cost_microusd > 10_000_000_000
        || caps.max_time_secs == 0
        || caps.max_time_secs > 31 * 24 * 60 * 60
    {
        return Err(format!("schedule schema: {label} is outside bounded budget limits").into());
    }
    Ok(())
}

fn aggregate_budget_within(
    label: &str,
    value: &AggregateBudgetCapsV1,
    maximum: &AggregateBudgetCapsV1,
) -> Result<(), BoxError> {
    aggregate_budget_caps(label, value)?;
    if value.max_attempts > maximum.max_attempts
        || value.max_tokens > maximum.max_tokens
        || value.max_cost_microusd > maximum.max_cost_microusd
        || value.max_time_secs > maximum.max_time_secs
    {
        return Err(format!("schedule schema: {label} exceeds its aggregate ceiling").into());
    }
    Ok(())
}

fn validate_grant_budgets(
    value: &GrantBudgetPolicyV1,
    case_ids: &BTreeSet<&str>,
    provider_families: &BTreeSet<&str>,
    triggers: &BTreeSet<TriggerKindV1>,
) -> Result<(), BoxError> {
    aggregate_budget_caps("UTC-day budget", &value.utc_day)?;
    aggregate_budget_within("rolling-24h budget", &value.rolling_24h, &value.utc_day)?;
    for (label, caps) in [
        ("protected scheduled budget", &value.protected_scheduled),
        ("protected test-merge budget", &value.protected_test_merge),
        ("manual-unallocated budget", &value.manual_unallocated),
    ] {
        aggregate_budget_within(label, caps, &value.utc_day)?;
    }

    let observed_cases = value
        .per_case
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    if &observed_cases != case_ids || observed_cases.len() != value.per_case.len() {
        return Err("schedule schema: per-case budgets must exactly cover grant case ids".into());
    }
    for item in &value.per_case {
        stable_id("budget case id", &item.id)?;
        aggregate_budget_within("per-case budget", &item.caps, &value.utc_day)?;
    }

    let observed_providers = value
        .per_provider
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    if &observed_providers != provider_families
        || observed_providers.len() != value.per_provider.len()
    {
        return Err(
            "schedule schema: per-provider budgets must exactly cover grant provider families"
                .into(),
        );
    }
    for item in &value.per_provider {
        stable_id("budget provider family", &item.id)?;
        aggregate_budget_within("per-provider budget", &item.caps, &value.utc_day)?;
    }

    let observed_triggers = value
        .per_trigger_pool
        .iter()
        .map(|item| item.trigger)
        .collect::<BTreeSet<_>>();
    if &observed_triggers != triggers || observed_triggers.len() != value.per_trigger_pool.len() {
        return Err(
            "schedule schema: per-trigger budgets must exactly cover grant triggers".into(),
        );
    }
    for item in &value.per_trigger_pool {
        aggregate_budget_within("per-trigger budget", &item.caps, &value.utc_day)?;
    }
    Ok(())
}

fn validate_source(source: &ProfileSourceRefV1) -> Result<(), BoxError> {
    if source.schema_version != 1 {
        return Err("schedule schema: profile source schema_version must be 1".into());
    }
    sha256("profile source", &source.source_sha256)?;
    sha256("profile row", &source.row_sha256)?;
    stable_id("profile row id", &source.row_id)
}

fn validate_authority(authority: &AdmissionAuthorityV1) -> Result<(), BoxError> {
    match authority {
        AdmissionAuthorityV1::CharacterizationOnce(value) => {
            stable_id("batch authorization id", &value.batch_authorization_id)?;
            sha256("batch authorization", &value.batch_authorization_sha256)?;
            stable_id("one-shot entry id", &value.entry_id)?;
            sha256("one-shot entry", &value.entry_sha256)?;
            stable_id("consumption nonce", &value.consumption_nonce)?;
            if value.generation == 0 {
                return Err("schedule schema: one-shot generation must be positive".into());
            }
        }
        AdmissionAuthorityV1::StandingGrant(value) => {
            stable_id("grant id", &value.grant_id)?;
            sha256("grant", &value.grant_sha256)?;
            stable_id("characterization id", &value.characterization_id)?;
            sha256("characterization", &value.characterization_sha256)?;
            if value.generation == 0 {
                return Err("schedule schema: standing-grant generation must be positive".into());
            }
        }
        AdmissionAuthorityV1::ManualAcknowledgement(value) => {
            sha256("manual admission", &value.manual_admission_sha256)?;
            stable_id("request nonce", &value.request_nonce)?;
        }
    }
    Ok(())
}

trait ValidateRecord {
    fn validate(&self) -> Result<(), BoxError>;
}

impl ValidateRecord for CaseExecutionFingerprintRecordV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.input.schema_version != 1 {
            return Err("schedule schema: case-execution fingerprint versions must be 1".into());
        }
        fingerprint(
            "case-execution characterization profile",
            &self.input.characterization_profile,
        )?;
        validate_execution_target(&self.input.target)?;
        sha256("candidate binary", &self.input.candidate.sha256)?;
        sha256(
            "candidate build provenance",
            &self.input.candidate.build_provenance_sha256,
        )?;
        if self.input.candidate.length_bytes == 0
            || self.input.candidate.length_bytes > MAX_CANDIDATE_BYTES
        {
            return Err("schedule schema: candidate length is outside the bounded range".into());
        }
        for (label, value) in [
            ("execution source", &self.input.bindings.source_sha256),
            ("execution row", &self.input.bindings.row_sha256),
            (
                "execution run manifest",
                &self.input.bindings.run_manifest_sha256,
            ),
            (
                "execution generated config",
                &self.input.bindings.generated_config_sha256,
            ),
            ("execution pin set", &self.input.bindings.pin_set_sha256),
            (
                "execution package integrity",
                &self.input.bindings.package_integrity_sha256,
            ),
            (
                "execution environment",
                &self.input.bindings.environment_sha256,
            ),
            (
                "execution prerequisites",
                &self.input.bindings.prerequisites_sha256,
            ),
        ] {
            sha256(label, value)?;
        }
        optional_sha256(
            "execution resolution bundle",
            &self.input.bindings.resolution_bundle,
        )?;
        optional_sha256("execution image", &self.input.bindings.image_digest)?;
        optional_sha256(
            "execution base image",
            &self.input.bindings.base_image_digest,
        )?;
        effective_identity("requested identity", &self.input.requested_identity)?;
        effective_identity(
            "expected effective identity",
            &self.input.expected_effective_identity,
        )?;
        self.input
            .actual_caps
            .validate("case-execution actual caps")?;
        fingerprint("case-execution fingerprint", &self.fingerprint)?;
        let expected = canonical_input_sha256("case-execution input", &self.input)?;
        if self.fingerprint.sha256 != expected {
            return Err(format!(
                "schedule schema: case-execution fingerprint mismatch; expected {expected}"
            )
            .into());
        }
        Ok(())
    }
}

impl ValidateRecord for AdmissionAttemptFingerprintRecordV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.input.schema_version != 1 {
            return Err("schedule schema: admission-attempt fingerprint versions must be 1".into());
        }
        fingerprint(
            "admission characterization profile",
            &self.input.characterization_profile,
        )?;
        fingerprint("admission case execution", &self.input.case_execution)?;
        validate_authority(&self.input.authority)?;
        validate_trigger_identity(&self.input.trigger)?;
        let authority_matches = matches!(
            (&self.input.authority, self.input.trigger.kind),
            (
                AdmissionAuthorityV1::CharacterizationOnce(_),
                TriggerKindV1::ManualCharacterization
            ) | (
                AdmissionAuthorityV1::ManualAcknowledgement(_),
                TriggerKindV1::ManualCompatibility
            ) | (AdmissionAuthorityV1::StandingGrant(_), TriggerKindV1::Daily)
                | (
                    AdmissionAuthorityV1::StandingGrant(_),
                    TriggerKindV1::ScheduledMain
                )
                | (
                    AdmissionAuthorityV1::StandingGrant(_),
                    TriggerKindV1::TestMerge
                )
        );
        if !authority_matches {
            return Err("schedule schema: admission authority does not match its trigger".into());
        }
        fingerprint("admission-attempt fingerprint", &self.fingerprint)?;
        let expected = canonical_input_sha256("admission-attempt input", &self.input)?;
        if self.fingerprint.sha256 != expected {
            return Err(format!(
                "schedule schema: admission-attempt fingerprint mismatch; expected {expected}"
            )
            .into());
        }
        Ok(())
    }
}

impl ValidateRecord for CharacterizationAuthorizationV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.entries.is_empty() || self.entries.len() > MAX_ITEMS {
            return Err("schedule schema: characterization authorization must be version 1 with bounded entries".into());
        }
        stable_id("authorization id", &self.authorization_id)?;
        for (label, value) in [
            ("authorization", &self.authorization_sha256),
            ("host identity", &self.host_identity_sha256),
            ("profile policy bundle", &self.profile_policy_bundle_sha256),
            ("scheduler binary", &self.scheduler_binary_sha256),
            ("price snapshot", &self.price_snapshot_sha256),
            ("legacy inventory", &self.legacy_inventory_sha256),
        ] {
            sha256(label, value)?;
        }
        bounded_text("operator", &self.operator)?;
        stable_id("environment owner", &self.environment_owner)?;
        if self.issued_at_ms <= 0 {
            return Err("schedule schema: authorization issued_at_ms must be positive".into());
        }
        unique_ids(
            "one-shot entry id",
            self.entries.iter().map(|entry| entry.entry_id.as_str()),
        )?;
        let mut profiles = BTreeSet::new();
        for entry in &self.entries {
            if entry.generation == 0 || entry.revocation_generation == 0 {
                return Err("schedule schema: one-shot entry generations must be positive".into());
            }
            sha256("one-shot entry", &entry.entry_sha256)?;
            stable_id("consumption nonce", &entry.consumption_nonce)?;
            validate_source(&entry.source)?;
            fingerprint("characterization profile", &entry.characterization_profile)?;
            fingerprint(
                "characterization execution",
                &entry.characterization_execution,
            )?;
            effective_identity(
                "one-shot proposed identity",
                &entry.proposed_effective_identity,
            )?;
            stable_id("provider family", &entry.provider_family)?;
            effects("one-shot effects", &entry.allowed_effects)?;
            entry.caps.validate("one-shot caps")?;
            if entry.command != "compatibility characterize" {
                return Err(
                    "schedule schema: one-shot command must be compatibility characterize".into(),
                );
            }
            time_range(
                "one-shot authority",
                entry.not_before_ms,
                entry.expires_at_ms,
            )?;
            if !profiles.insert(entry.characterization_profile.sha256.as_str()) {
                return Err(
                    "schedule schema: one authorization cannot contain duplicate live profiles"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

impl ValidateRecord for ProviderEffectGrantV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.profiles.is_empty() || self.profiles.len() > MAX_ITEMS {
            return Err(
                "schedule schema: provider grant must be version 1 with bounded profiles".into(),
            );
        }
        stable_id("grant id", &self.grant_id)?;
        bounded_text("grant operator", &self.operator)?;
        stable_id("grant environment owner", &self.environment_owner)?;
        if self.generation == 0 || self.revocation_generation == 0 {
            return Err("schedule schema: provider grant generations must be positive".into());
        }
        for (label, value) in [
            ("grant", &self.grant_sha256),
            ("host identity", &self.host_identity_sha256),
            ("profile policy bundle", &self.profile_policy_bundle_sha256),
            ("scheduler binary", &self.scheduler_binary_sha256),
            ("price snapshot", &self.price_snapshot_sha256),
            ("legacy inventory", &self.legacy_inventory_sha256),
        ] {
            sha256(label, value)?;
        }
        time_range("provider grant", self.not_before_ms, self.expires_at_ms)?;
        time_range(
            "price snapshot",
            self.price_snapshot_observed_at_ms,
            self.price_snapshot_valid_until_ms,
        )?;
        if self.expires_at_ms > self.price_snapshot_valid_until_ms {
            return Err("schedule schema: provider grant outlives its price snapshot".into());
        }
        self.per_run_caps.validate("provider grant per-run caps")?;
        effects("provider grant effects", &self.allowed_effects)?;
        if self.confirmation_allowance > 1 {
            return Err(
                "schedule schema: provider grant confirmation_allowance must be 0 or 1".into(),
            );
        }
        if self.triggers.is_empty()
            || self.triggers.iter().collect::<BTreeSet<_>>().len() != self.triggers.len()
            || self.triggers.iter().any(|trigger| {
                matches!(
                    trigger,
                    TriggerKindV1::ManualCharacterization | TriggerKindV1::ManualCompatibility
                )
            })
        {
            return Err(
                "schedule schema: standing grant triggers must be unique and unattended-only"
                    .into(),
            );
        }
        unique_ids("grant case id", self.case_ids.iter().map(String::as_str))?;
        unique_ids(
            "grant provider family",
            self.provider_families.iter().map(String::as_str),
        )?;
        let trigger_set = self.triggers.iter().copied().collect::<BTreeSet<_>>();
        let case_set = self
            .case_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let provider_set = self
            .provider_families
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        validate_grant_budgets(&self.budgets, &case_set, &provider_set, &trigger_set)?;
        unique_ids(
            "launchd label",
            self.launchd.iter().map(|item| item.label.as_str()),
        )?;
        for item in &self.launchd {
            sha256("launchd plist", &item.plist_sha256)?;
            if !self.triggers.contains(&item.trigger) {
                return Err(
                    "schedule schema: launchd binding names a trigger outside the grant".into(),
                );
            }
        }
        let mut profiles = BTreeSet::new();
        let mut profile_cases = BTreeSet::new();
        let mut profile_providers = BTreeSet::new();
        for profile in &self.profiles {
            stable_id("granted case id", &profile.case_id)?;
            stable_id("granted provider family", &profile.provider_family)?;
            validate_source(&profile.source)?;
            fingerprint("granted profile", &profile.characterization_profile)?;
            stable_id("characterization id", &profile.characterization_id)?;
            sha256("characterization", &profile.characterization_sha256)?;
            effective_identity("granted effective identity", &profile.effective_identity)?;
            profile.caps.validate("granted profile caps")?;
            profile
                .caps
                .within(&self.per_run_caps, "granted profile caps")?;
            if !profiles.insert(profile.characterization_profile.sha256.as_str()) {
                return Err("schedule schema: provider grant repeats a profile".into());
            }
            if !profile_cases.insert(profile.case_id.as_str()) {
                return Err(
                    "schedule schema: each grant case must have exactly one characterized profile"
                        .into(),
                );
            }
            profile_providers.insert(profile.provider_family.as_str());
        }
        if profile_cases != case_set || profile_providers != provider_set {
            return Err(
                "schedule schema: characterized profiles do not cover the grant case/provider sets"
                    .into(),
            );
        }
        Ok(())
    }
}

impl ValidateRecord for ManualAdmissionV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || !self.acknowledged_billable {
            return Err(
                "schedule schema: manual admission must be version 1 with explicit acknowledgement"
                    .into(),
            );
        }
        stable_id("manual request nonce", &self.request_nonce)?;
        bounded_text("manual operator", &self.operator)?;
        stable_id("manual environment owner", &self.environment_owner)?;
        sha256("scheduler binary", &self.scheduler_binary_sha256)?;
        sha256("manual input source", &self.input_source_sha256)?;
        fingerprint("manual profile", &self.characterization_profile)?;
        fingerprint("manual execution", &self.case_execution)?;
        stable_id("manual freshness bucket", &self.freshness_bucket)?;
        bounded_text("manual command", &self.command)?;
        if self.command.contains("serve") || self.command.contains("schedule-tick") {
            return Err(
                "schedule schema: manual admission cannot originate from serve or a timer".into(),
            );
        }
        self.caps.validate("manual caps")?;
        effects("manual effects", &self.allowed_effects)?;
        if self.retry_cap != 0 || self.fallback_cap != 0 {
            return Err("schedule schema: manual admission retry/fallback must be zero".into());
        }
        time_range("manual admission", self.issued_at_ms, self.expires_at_ms)
    }
}

impl ValidateRecord for StorageConsentV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.evidence_classes.is_empty() {
            return Err(
                "schedule schema: storage consent must be version 1 with evidence scope".into(),
            );
        }
        stable_id("storage consent id", &self.consent_id)?;
        sha256("storage consent", &self.consent_sha256)?;
        bounded_text("storage operator", &self.operator)?;
        stable_id("storage environment owner", &self.environment_owner)?;
        if self.cold_root != "~/Documents/a2a-bridge/evidence-archive" {
            return Err("schedule schema: storage consent cold root is not owner-approved".into());
        }
        if self.revocation_generation == 0 {
            return Err(
                "schedule schema: storage consent revocation generation must be positive".into(),
            );
        }
        bounded_text("FileProvider domain", &self.file_provider_domain_id)?;
        if self.evidence_classes.len() > MAX_ITEMS
            || self.evidence_classes.iter().collect::<BTreeSet<_>>().len()
                != self.evidence_classes.len()
        {
            return Err("schedule schema: evidence classes must be bounded and unique".into());
        }
        time_range("storage consent", self.not_before_ms, self.expires_at_ms)
    }
}

impl ValidateRecord for CharacterizationRecordV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 {
            return Err("schedule schema: characterization schema_version must be 1".into());
        }
        stable_id("characterization id", &self.characterization_id)?;
        validate_source(&self.source)?;
        sha256("profile policy bundle", &self.profile_policy_bundle_sha256)?;
        fingerprint("characterization profile", &self.characterization_profile)?;
        fingerprint("case execution", &self.case_execution)?;
        fingerprint("admission attempt", &self.admission_attempt)?;
        validate_authority(&self.authority)?;
        if !matches!(
            self.authority,
            AdmissionAuthorityV1::CharacterizationOnce(_)
        ) {
            return Err(
                "schedule schema: characterization requires one-shot characterization authority"
                    .into(),
            );
        }
        effective_identity(
            "characterization expected identity",
            &self.expected_effective_identity,
        )?;
        effective_identity(
            "characterization observed identity",
            &self.observed_effective_identity,
        )?;
        sha256("characterization evidence", &self.evidence_sha256)?;
        if self.expected_effective_identity != self.observed_effective_identity
            && self.outcome != CharacterizationOutcomeV1::CharacterizationInconclusive
        {
            return Err("schedule schema: effective-identity mismatch must be inconclusive".into());
        }
        if self.terminal_at_ms <= 0 {
            return Err("schedule schema: characterization terminal time must be positive".into());
        }
        Ok(())
    }
}

impl ValidateRecord for SafetyHoldV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.created_at_ms <= 0 {
            return Err(
                "schedule schema: safety hold must be version 1 with positive creation time".into(),
            );
        }
        stable_id("hold id", &self.hold_id)?;
        fingerprint("held profile", &self.characterization_profile)?;
        fingerprint("held execution", &self.case_execution)?;
        match (self.cleared_at_ms, self.clearance_reason.as_deref()) {
            (None, None) => Ok(()),
            (Some(time), Some(reason)) if time >= self.created_at_ms => {
                bounded_text("hold clearance reason", reason)
            }
            _ => Err("schedule schema: hold clearance time and reason must appear together".into()),
        }
    }
}

impl ValidateRecord for QuarantineV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 {
            return Err("schedule schema: quarantine schema_version must be 1".into());
        }
        stable_id("quarantine id", &self.quarantine_id)?;
        fingerprint("quarantined profile", &self.profile)?;
        bounded_text("quarantine reason", &self.reason)?;
        time_range("quarantine", self.created_at_ms, self.expires_at_ms)?;
        if !self.active {
            return Err("schedule schema: an inactive quarantine must be represented by a separate closure record".into());
        }
        Ok(())
    }
}

impl ValidateRecord for FailureDispositionV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.identical_complete_occurrences == 0 {
            return Err("schedule schema: failure disposition must be version 1 with at least one occurrence".into());
        }
        fingerprint("failure profile", &self.characterization_profile)?;
        fingerprint("failure execution", &self.case_execution)?;
        sha256("failure evidence", &self.evidence_sha256)?;
        stable_id("typed failure code", &self.typed_code)?;
        if self.first_seen_ms <= 0 || self.last_seen_ms < self.first_seen_ms {
            return Err("schedule schema: failure observation times are invalid".into());
        }
        let valid = matches!(
            (
                self.failure_kind,
                self.identical_complete_occurrences,
                self.action
            ),
            (
                FailureKindV1::TypedImmutable,
                _,
                FailureActionV1::Suppressed
            ) | (
                FailureKindV1::TypedTransient,
                1,
                FailureActionV1::ConfirmationDue
            ) | (
                FailureKindV1::TypedTransient,
                2..,
                FailureActionV1::Suppressed
            ) | (
                FailureKindV1::UntypedTransient,
                1,
                FailureActionV1::ConfirmationDue
            ) | (
                FailureKindV1::CandidateUnknown,
                _,
                FailureActionV1::UnknownRetained
            ) | (_, _, FailureActionV1::Recovered)
        );
        if !valid {
            return Err("schedule schema: failure kind/count/action violates the confirmation and suppression policy".into());
        }
        Ok(())
    }
}

fn validate_test_merge(target: &TestMergeIdentityV1) -> Result<(), BoxError> {
    bounded_text("repository", &target.repository)?;
    if target.pull_request == 0
        || target.merge_ref != format!("refs/pull/{}/merge", target.pull_request)
        || target.ordered_parents.len() != 2
    {
        return Err("schedule schema: test-merge identity is not canonical".into());
    }
    for (label, value) in [
        ("base", &target.base_sha256),
        ("head", &target.head_sha256),
        ("merge", &target.merge_sha256),
        ("tree", &target.tree_sha256),
    ] {
        sha256(label, value)?;
    }
    if target.ordered_parents != [target.base_sha256.clone(), target.head_sha256.clone()] {
        return Err("schedule schema: test-merge ordered parents must be base then head".into());
    }
    Ok(())
}

impl ValidateRecord for ImpactRecordV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 {
            return Err("schedule schema: impact schema_version must be 1".into());
        }
        sha256("classifier", &self.classifier_sha256)?;
        validate_test_merge(&self.target)?;
        if self.classes.is_empty()
            || self.classes.iter().collect::<BTreeSet<_>>().len() != self.classes.len()
        {
            return Err("schedule schema: impact classes must be non-empty and unique".into());
        }
        unique_ids("due case id", self.due_case_ids.iter().map(String::as_str))?;
        if self.no_impact_proved != self.due_case_ids.is_empty() {
            return Err("schedule schema: no-impact proof and due cases contradict".into());
        }
        Ok(())
    }
}

impl ValidateRecord for LedgerRecordV1 {
    fn validate(&self) -> Result<(), BoxError> {
        match self {
            LedgerRecordV1::Reservation(value) => {
                if value.schema_version != 1 {
                    return Err(
                        "schedule schema: ledger reservation schema_version must be 1".into(),
                    );
                }
                stable_id("ledger reservation id", &value.reservation_id)?;
                sha256("attempt idempotency key", &value.attempt_idempotency_key)?;
                fingerprint("ledger profile", &value.characterization_profile)?;
                fingerprint("ledger execution", &value.case_execution)?;
                fingerprint("ledger admission attempt", &value.admission_attempt)?;
                validate_authority(&value.authority)?;
                sha256("ledger equivalent-work key", &value.equivalent_work_key)?;
                stable_id("ledger freshness bucket", &value.freshness_bucket)?;
                optional_stable_id("ledger repeat nonce", &value.repeat_nonce)?;
                value.caps.validate("ledger reservation caps")?;
                stable_id("UTC day id", &value.utc_day_id)?;
                stable_id("rolling window id", &value.rolling_window_id)?;
                if value.reserved_at_ms <= 0 {
                    return Err("schedule schema: ledger reservation time must be positive".into());
                }
            }
            LedgerRecordV1::Reconciliation(value) => {
                if value.schema_version != 1 {
                    return Err(
                        "schedule schema: ledger reconciliation schema_version must be 1".into(),
                    );
                }
                stable_id("ledger reservation id", &value.reservation_id)?;
                fingerprint("reconciled profile", &value.characterization_profile)?;
                fingerprint("reconciled execution", &value.case_execution)?;
                fingerprint("reconciled admission", &value.admission_attempt)?;
                validate_authority(&value.authority)?;
                sha256("reconciled equivalent-work key", &value.equivalent_work_key)?;
                sha256("terminal evidence", &value.terminal_evidence_sha256)?;
                let usage = &value.charged_usage;
                if usage.attempts > 1
                    || usage.tokens > 1_000_000
                    || usage.cost_microusd > 100_000_000
                    || usage.elapsed_millis > 31 * 24 * 60 * 60 * 1000
                {
                    return Err("schedule schema: ledger charged usage exceeds hard bounds".into());
                }
                match value.disposition {
                    LedgerDispositionV1::ReleasedPreEffect
                        if usage.attempts == 0
                            && usage.tokens == 0
                            && usage.cost_microusd == 0
                            && usage.elapsed_millis == 0
                            && !value.prompt_may_have_been_accepted => {}
                    LedgerDispositionV1::ChargedTerminal
                    | LedgerDispositionV1::ChargedConservative
                        if usage.attempts == 1 => {}
                    _ => {
                        return Err(
                            "schedule schema: ledger disposition and charged usage disagree".into(),
                        )
                    }
                }
                if value.reconciled_at_ms <= 0 {
                    return Err(
                        "schedule schema: ledger reconciliation time must be positive".into(),
                    );
                }
            }
        }
        Ok(())
    }
}

impl ValidateRecord for EquivalentWorkReservationV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.reserved_at_ms <= 0 {
            return Err(
                "schedule schema: equivalent-work reservation must be version 1 with positive time"
                    .into(),
            );
        }
        stable_id("equivalent reservation id", &self.reservation_id)?;
        sha256("equivalent work key", &self.equivalent_work_key)?;
        stable_id("freshness bucket", &self.freshness_bucket)?;
        fingerprint("equivalent profile", &self.characterization_profile)?;
        fingerprint("equivalent execution", &self.case_execution)?;
        fingerprint("equivalent admission", &self.admission_attempt)?;
        validate_authority(&self.authority)
    }
}

impl ValidateRecord for ConsumptionRecordV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.consumed_at_ms <= 0 {
            return Err("schedule schema: consumption must be version 1 with positive time".into());
        }
        stable_id("consumption id", &self.consumption_id)?;
        sha256("equivalent work key", &self.equivalent_work_key)?;
        sha256("consumed evidence", &self.evidence_sha256)?;
        fingerprint("consumed profile", &self.characterization_profile)?;
        fingerprint("consumed execution", &self.case_execution)?;
        fingerprint("consumption admission", &self.admission_attempt)?;
        validate_authority(&self.authority)?;
        let purpose_allowed = self.requested_purpose == self.satisfied_purpose
            || (self.requested_purpose == EvidencePurposeV1::ProviderPathAdvisory
                && self.satisfied_purpose == EvidencePurposeV1::ClaimedSupportGate);
        if !purpose_allowed || self.requested_purpose == EvidencePurposeV1::Characterization {
            return Err("schedule schema: evidence-purpose reuse is not equal-or-stronger".into());
        }
        Ok(())
    }
}

impl ValidateRecord for ScheduledExecutionSourceV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.source.kind != ProfileSourceKindV1::ScheduledAdvisory {
            return Err("schedule schema: scheduled source must be version 1 and advisory".into());
        }
        validate_source(&self.source)?;
        for (label, value) in [
            ("scheduled source", &self.source_sha256),
            ("profile policy bundle", &self.profile_policy_bundle_sha256),
        ] {
            sha256(label, value)?;
        }
        fingerprint("scheduled profile", &self.characterization_profile)?;
        self.case_execution.validate()?;
        self.admission_attempt.validate()?;
        effective_identity("scheduled requested identity", &self.requested_identity)?;
        effective_identity(
            "scheduled expected identity",
            &self.expected_effective_identity,
        )?;
        self.caps.validate("scheduled source caps")?;
        if self.retry_cap != 0 || self.fallback_cap != 0 {
            return Err("schedule schema: scheduled source retry/fallback must be zero".into());
        }
        match (&self.authority, self.trigger) {
            (
                AdmissionAuthorityV1::CharacterizationOnce(_),
                TriggerKindV1::ManualCharacterization,
            )
            | (AdmissionAuthorityV1::StandingGrant(_), TriggerKindV1::Daily)
            | (AdmissionAuthorityV1::StandingGrant(_), TriggerKindV1::ScheduledMain)
            | (AdmissionAuthorityV1::StandingGrant(_), TriggerKindV1::TestMerge) => {}
            _ => {
                return Err(
                    "schedule schema: scheduled source authority does not match its trigger".into(),
                )
            }
        }
        validate_authority(&self.authority)?;
        if self.case_execution.input.characterization_profile != self.characterization_profile
            || self.admission_attempt.input.characterization_profile
                != self.characterization_profile
            || self.admission_attempt.input.case_execution != self.case_execution.fingerprint
            || self.admission_attempt.input.authority != self.authority
            || self.admission_attempt.input.trigger.kind != self.trigger
            || self.case_execution.input.bindings.source_sha256 != self.source.source_sha256
            || self.case_execution.input.bindings.row_sha256 != self.source.row_sha256
            || self.case_execution.input.requested_identity != self.requested_identity
            || self.case_execution.input.expected_effective_identity
                != self.expected_effective_identity
            || self.case_execution.input.actual_caps != self.caps
        {
            return Err(
                "schedule schema: scheduled source contains inconsistent canonical bindings".into(),
            );
        }
        Ok(())
    }
}

impl ValidateRecord for ClaimedSupportCharacterizationSourceV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1
            || self.source.kind != ProfileSourceKindV1::ClaimedSupportGate
            || self.trigger != TriggerKindV1::ManualCharacterization
            || !matches!(
                self.authority,
                AdmissionAuthorityV1::CharacterizationOnce(_)
            )
        {
            return Err("schedule schema: claimed-support characterization requires one-shot manual characterization authority".into());
        }
        validate_source(&self.source)?;
        for (label, value) in [
            ("claimed-support source", &self.source_sha256),
            ("production manifest", &self.production_manifest_sha256),
            ("profile policy bundle", &self.profile_policy_bundle_sha256),
            ("pinned config", &self.pinned_config_sha256),
        ] {
            sha256(label, value)?;
        }
        fingerprint("claimed-support profile", &self.characterization_profile)?;
        self.characterization_execution.validate()?;
        self.admission_attempt.validate()?;
        effective_identity(
            "claimed-support requested identity",
            &self.requested_identity,
        )?;
        effective_identity(
            "claimed-support expected identity",
            &self.expected_effective_identity,
        )?;
        self.caps.validate("claimed-support caps")?;
        validate_authority(&self.authority)?;
        if self.source.source_sha256 != self.production_manifest_sha256
            || self
                .characterization_execution
                .input
                .characterization_profile
                != self.characterization_profile
            || self.admission_attempt.input.characterization_profile
                != self.characterization_profile
            || self.admission_attempt.input.case_execution
                != self.characterization_execution.fingerprint
            || self.admission_attempt.input.authority != self.authority
            || self.admission_attempt.input.trigger.kind != self.trigger
            || self.characterization_execution.input.bindings.source_sha256
                != self.source.source_sha256
            || self.characterization_execution.input.bindings.row_sha256 != self.source.row_sha256
            || self
                .characterization_execution
                .input
                .bindings
                .generated_config_sha256
                != self.pinned_config_sha256
            || self.characterization_execution.input.requested_identity != self.requested_identity
            || self
                .characterization_execution
                .input
                .expected_effective_identity
                != self.expected_effective_identity
            || self.characterization_execution.input.actual_caps != self.caps
        {
            return Err(
                "schedule schema: claimed-support source contains inconsistent canonical bindings"
                    .into(),
            );
        }
        Ok(())
    }
}

impl ValidateRecord for ScheduleEvidenceRecordV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.created_at_ms <= 0 {
            return Err(
                "schedule schema: schedule sidecar must be version 1 with positive creation time"
                    .into(),
            );
        }
        stable_id("schedule record id", &self.schedule_record_id)?;
        validate_source(&self.source)?;
        sha256("profile policy bundle", &self.profile_policy_bundle_sha256)?;
        fingerprint("sidecar profile", &self.characterization_profile)?;
        fingerprint("sidecar execution", &self.case_execution)?;
        fingerprint("sidecar admission", &self.admission_attempt)?;
        validate_authority(&self.authority)?;
        optional_sha256("aggregate", &self.aggregate)?;
        stable_id("evidence index id", &self.evidence_index_id)?;
        optional_record_ref("storage consent", &self.storage_consent)?;
        optional_record_ref("quarantine", &self.quarantine)?;
        optional_record_ref("characterization", &self.characterization)?;
        stable_id("sidecar window id", &self.window_id)?;
        sha256(
            "sidecar attempt idempotency key",
            &self.attempt_idempotency_key,
        )?;
        sha256("sidecar equivalent-work key", &self.equivalent_work_key)?;
        optional_record_ref("consumption", &self.consumption)?;
        optional_stable_id("sidecar repeat nonce", &self.repeat_nonce)?;
        stable_id("ledger reservation id", &self.ledger_reservation_id)?;
        for (label, value) in [
            ("budget reservation", &self.budget_reservation_sha256),
            ("deadline derivation", &self.deadline_derivation_sha256),
            ("preflight results", &self.preflight_results_sha256),
            ("admission lock holder", &self.admission_lock_holder_sha256),
            ("supervisor record", &self.supervisor_record_sha256),
            ("freshness observation", &self.freshness_observation_sha256),
        ] {
            sha256(label, value)?;
        }
        optional_sha256("ledger reconciliation", &self.ledger_reconciliation)?;
        effective_identity("sidecar requested identity", &self.requested_identity)?;
        effective_identity(
            "sidecar expected effective identity",
            &self.expected_effective_identity,
        )?;
        optional_effective_identity(
            "sidecar observed effective identity",
            &self.observed_effective_identity,
        )?;
        optional_record_ref("publication outbox", &self.publication_outbox)?;
        optional_sha256("status publication", &self.status_publication)?;
        unique_ids(
            "affected case id",
            self.affected_case_ids.iter().map(String::as_str),
        )?;
        if self.affected_case_ids.is_empty() {
            return Err("schedule schema: sidecar must bind at least one affected case".into());
        }
        match (&self.check, self.trigger) {
            (
                CheckBindingV1::TestMergeResult {
                    target,
                    guarded_observation_sha256,
                    classifier_sha256,
                    required_rule_sha256,
                    context,
                    expected_source_sha256,
                },
                TriggerKindV1::TestMerge,
            ) => {
                validate_test_merge(target)?;
                for (label, value) in [
                    ("guarded observation", guarded_observation_sha256),
                    ("impact classifier", classifier_sha256),
                    ("required rule", required_rule_sha256),
                    ("expected check source", expected_source_sha256),
                ] {
                    sha256(label, value)?;
                }
                bounded_text("required check context", context)?;
            }
            (
                CheckBindingV1::Absent,
                TriggerKindV1::Daily
                | TriggerKindV1::ScheduledMain
                | TriggerKindV1::ManualCharacterization
                | TriggerKindV1::ManualCompatibility,
            ) => {}
            _ => {
                return Err(
                    "schedule schema: sidecar check scope/test merge/trigger disagree".into(),
                )
            }
        }
        let authority_matches = matches!(
            (&self.authority, self.trigger),
            (
                AdmissionAuthorityV1::CharacterizationOnce(_),
                TriggerKindV1::ManualCharacterization
            ) | (
                AdmissionAuthorityV1::ManualAcknowledgement(_),
                TriggerKindV1::ManualCompatibility
            ) | (AdmissionAuthorityV1::StandingGrant(_), TriggerKindV1::Daily)
                | (
                    AdmissionAuthorityV1::StandingGrant(_),
                    TriggerKindV1::ScheduledMain
                )
                | (
                    AdmissionAuthorityV1::StandingGrant(_),
                    TriggerKindV1::TestMerge
                )
        );
        if !authority_matches {
            return Err("schedule schema: sidecar authority does not match its trigger".into());
        }
        Ok(())
    }
}

impl ValidateRecord for PublicationOutboxV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.pull_request == 0 {
            return Err("schedule schema: publication outbox must be version 1 with a PR".into());
        }
        stable_id("outbox id", &self.outbox_id)?;
        sha256("outbox test merge", &self.test_merge_sha256)?;
        bounded_text("check context", &self.context)?;
        stable_id("App id", &self.app_id)?;
        stable_id("external id", &self.external_id)?;
        let terminal = matches!(
            self.state,
            PublicationOutboxStateV1::Prepared
                | PublicationOutboxStateV1::UpdateUnknown
                | PublicationOutboxStateV1::RemotelyObserved
                | PublicationOutboxStateV1::Confirmed
        );
        let terminal_fields = [
            matches!(
                self.terminal_consumption,
                OptionalStableIdV1::StableId { .. }
            ),
            matches!(
                self.desired_conclusion,
                OptionalCheckConclusionV1::Conclusion { .. }
            ),
            matches!(self.evidence_set, OptionalSha256V1::Sha256 { .. }),
            matches!(self.final_guard, OptionalSha256V1::Sha256 { .. }),
        ];
        if (terminal && terminal_fields.iter().any(|present| !present))
            || (!terminal && terminal_fields.iter().any(|present| *present))
        {
            return Err(
                "schedule schema: outbox terminal fields must appear exactly at prepared or later"
                    .into(),
            );
        }
        let check_run_bound = match &self.check_run {
            OptionalCheckRunIdV1::Absent => false,
            OptionalCheckRunIdV1::CheckRun { id } if *id > 0 => true,
            OptionalCheckRunIdV1::CheckRun { .. } => {
                return Err("schedule schema: check_run id must be positive".into())
            }
        };
        let check_run_state_matches = match self.state {
            PublicationOutboxStateV1::CreateIntent | PublicationOutboxStateV1::CreateUnknown => {
                !check_run_bound
            }
            _ => check_run_bound,
        };
        if !check_run_state_matches {
            return Err(
                "schedule schema: check_run binding disagrees with publication outbox state".into(),
            );
        }
        let remotely_observed = matches!(
            self.state,
            PublicationOutboxStateV1::RemotelyObserved | PublicationOutboxStateV1::Confirmed
        );
        let remote_observation_present =
            matches!(self.remote_observation, OptionalSha256V1::Sha256 { .. });
        if remotely_observed != remote_observation_present
            || (remotely_observed && self.remote_observation_attempts == 0)
            || self.remote_observation_attempts > 1_000
            || (self.state == PublicationOutboxStateV1::CreateIntent
                && self.remote_observation_attempts != 0)
        {
            return Err(
                "schedule schema: remote observation hash/attempts disagree with outbox state"
                    .into(),
            );
        }
        optional_stable_id("terminal consumption", &self.terminal_consumption)?;
        optional_sha256("evidence set", &self.evidence_set)?;
        optional_sha256("final guard", &self.final_guard)?;
        optional_sha256("remote observation", &self.remote_observation)?;
        Ok(())
    }
}

impl ValidateRecord for EvidenceIndexV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.entries.len() > MAX_ITEMS {
            return Err("schedule schema: evidence index must be version 1 and bounded".into());
        }
        stable_id("evidence index id", &self.index_id)?;
        unique_ids(
            "evidence id",
            self.entries.iter().map(|entry| entry.evidence_id.as_str()),
        )?;
        for entry in &self.entries {
            sha256("full evidence", &entry.full_evidence_sha256)?;
            sha256("compact evidence", &entry.compact_record_sha256)?;
            bounded_text("hot evidence path", &entry.hot_path)?;
            if let Some(path) = &entry.cold_path {
                bounded_text("cold evidence path", path)?;
            }
            if entry.full_retain_until_ms <= 0
                || entry.compact_retain_until_ms < entry.full_retain_until_ms
                || entry.lease_count > 1_000_000
            {
                return Err("schedule schema: evidence retention or lease state is invalid".into());
            }
        }
        Ok(())
    }
}

impl ValidateRecord for ScheduleStatusV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.generated_at_ms <= 0 || self.cases.len() > MAX_ITEMS {
            return Err(
                "schedule schema: status must be version 1, timestamped, and bounded".into(),
            );
        }
        sha256("status policy", &self.policy_sha256)?;
        sha256("ledger headroom", &self.ledger_headroom_sha256)?;
        if let Some(value) = &self.provider_grant_sha256 {
            sha256("provider grant", value)?;
        }
        if let Some(value) = &self.storage_consent_sha256 {
            sha256("storage consent", value)?;
        }
        bounded_text("storage state", &self.storage_state)?;
        if !matches!(
            self.fresh_one_shot_compatibility.as_str(),
            "pass" | "fail" | "unknown"
        ) || self.shared_operator_health != "not_evaluated"
        {
            return Err("schedule schema: status must keep one-shot compatibility separate from operator health".into());
        }
        unique_ids(
            "status case id",
            self.cases.iter().map(|case| case.case_id.as_str()),
        )?;
        Ok(())
    }
}

impl ValidateRecord for DogfoodRoutingPolicyV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || !self.advisory_only || !self.audit_required {
            return Err("schedule schema: dogfood routing must be advisory and audited".into());
        }
        let expected = BTreeSet::from([
            DogfoodTaskClassV1::BoundedSummaryDocsLightBrainstorm,
            DogfoodTaskClassV1::SmallSpecifiedImplementation,
            DogfoodTaskClassV1::NormalImplementation,
            DogfoodTaskClassV1::SpecDesignArchitectureAuthoring,
            DogfoodTaskClassV1::CleanroomSpecTechnicalDesign,
            DogfoodTaskClassV1::AdversarialDesignImplementationReview,
            DogfoodTaskClassV1::ReleaseCompatibilityReview,
            DogfoodTaskClassV1::FullBranchReview,
            DogfoodTaskClassV1::RequirementsBrainstormAnalysisGrooming,
            DogfoodTaskClassV1::ConcurrencyTransactionCriticalProof,
        ]);
        let actual = self
            .rules
            .iter()
            .map(|rule| rule.task_class)
            .collect::<BTreeSet<_>>();
        if actual != expected || actual.len() != self.rules.len() {
            return Err(
                "schedule schema: dogfood routing must contain each task class exactly once".into(),
            );
        }
        for rule in &self.rules {
            bounded_text("primary model class", &rule.primary_model_class)?;
            bounded_text("routing effort", &rule.effort)?;
            if let Some(value) = &rule.second_opinion {
                bounded_text("second opinion", value)?;
            }
            if rule.max_allowed
                != (rule.task_class == DogfoodTaskClassV1::ConcurrencyTransactionCriticalProof)
            {
                return Err("schedule schema: max is reserved for the concurrency/transaction/critical-proof class".into());
            }
            if !rule.fable_allowed_only_when_hard_complex {
                return Err(
                    "schedule schema: Fable must remain limited to hard/complex work".into(),
                );
            }
        }
        Ok(())
    }
}

fn parse_and_validate<T: DeserializeOwned + ValidateRecord>(
    bytes: &[u8],
    label: &str,
) -> Result<(), BoxError> {
    let value: T = serde_json::from_slice(bytes)
        .map_err(|error| format!("schedule schema: invalid {label}: {error}"))?;
    value.validate()
}

pub(super) fn validate_schedule_record(kind: &str, path: &Path) -> Result<(), BoxError> {
    let snapshot =
        local_file::read_regular_file_bounded(path, "schedule record", MAX_RECORD_BYTES)?;
    match kind {
        "case-execution-fingerprint" => {
            parse_and_validate::<CaseExecutionFingerprintRecordV1>(&snapshot.bytes, kind)
        }
        "admission-attempt-fingerprint" => {
            parse_and_validate::<AdmissionAttemptFingerprintRecordV1>(&snapshot.bytes, kind)
        }
        "characterization-authorization" => {
            parse_and_validate::<CharacterizationAuthorizationV1>(&snapshot.bytes, kind)
        }
        "provider-effect-grant" => {
            parse_and_validate::<ProviderEffectGrantV1>(&snapshot.bytes, kind)
        }
        "manual-admission" => parse_and_validate::<ManualAdmissionV1>(&snapshot.bytes, kind),
        "storage-consent" => parse_and_validate::<StorageConsentV1>(&snapshot.bytes, kind),
        "characterization" => parse_and_validate::<CharacterizationRecordV1>(&snapshot.bytes, kind),
        "safety-hold" => parse_and_validate::<SafetyHoldV1>(&snapshot.bytes, kind),
        "quarantine" => parse_and_validate::<QuarantineV1>(&snapshot.bytes, kind),
        "failure-disposition" => parse_and_validate::<FailureDispositionV1>(&snapshot.bytes, kind),
        "impact" => parse_and_validate::<ImpactRecordV1>(&snapshot.bytes, kind),
        "ledger" => parse_and_validate::<LedgerRecordV1>(&snapshot.bytes, kind),
        "equivalent-work" => {
            parse_and_validate::<EquivalentWorkReservationV1>(&snapshot.bytes, kind)
        }
        "consumption" => parse_and_validate::<ConsumptionRecordV1>(&snapshot.bytes, kind),
        "scheduled-source" => {
            parse_and_validate::<ScheduledExecutionSourceV1>(&snapshot.bytes, kind)
        }
        "claimed-support-characterization-source" => {
            parse_and_validate::<ClaimedSupportCharacterizationSourceV1>(&snapshot.bytes, kind)
        }
        "schedule-sidecar" => parse_and_validate::<ScheduleEvidenceRecordV1>(&snapshot.bytes, kind),
        "publication-outbox" => parse_and_validate::<PublicationOutboxV1>(&snapshot.bytes, kind),
        "evidence-index" => parse_and_validate::<EvidenceIndexV1>(&snapshot.bytes, kind),
        "status" => parse_and_validate::<ScheduleStatusV1>(&snapshot.bytes, kind),
        "routing" => parse_and_validate::<DogfoodRoutingPolicyV1>(&snapshot.bytes, kind),
        other => Err(format!("schedule schema: unknown record kind {other:?}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    fn fingerprint_value(ch: char) -> FingerprintV1 {
        FingerprintV1 {
            schema_version: 1,
            sha256: digest(ch),
        }
    }

    fn standing_authority() -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::StandingGrant(StandingGrantAuthorityV1 {
            grant_id: "grant-1".into(),
            generation: 1,
            grant_sha256: digest('a'),
            characterization_id: "characterization-1".into(),
            characterization_sha256: digest('b'),
        })
    }

    fn one_shot_authority() -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::CharacterizationOnce(CharacterizationOnceAuthorityV1 {
            batch_authorization_id: "authorization-1".into(),
            batch_authorization_sha256: digest('a'),
            entry_id: "entry-1".into(),
            generation: 1,
            entry_sha256: digest('b'),
            consumption_nonce: "nonce-1".into(),
        })
    }

    fn effect_caps() -> EffectCapsV1 {
        EffectCapsV1 {
            timeout_secs: 10,
            max_tokens: 10,
            max_cost_microusd: 10,
            attempts: 1,
            retry_cap: 0,
            fallback_cap: 0,
        }
    }

    fn aggregate_caps(attempts: u64) -> AggregateBudgetCapsV1 {
        AggregateBudgetCapsV1 {
            max_attempts: attempts,
            max_tokens: 1_000,
            max_cost_microusd: 1_000,
            max_time_secs: 1_000,
        }
    }

    fn provider_grant() -> ProviderEffectGrantV1 {
        let profile = CharacterizedGrantProfileV1 {
            case_id: "case-1".into(),
            provider_family: "provider-1".into(),
            source: ProfileSourceRefV1 {
                kind: ProfileSourceKindV1::ScheduledAdvisory,
                schema_version: 1,
                source_sha256: digest('1'),
                row_id: "case-1".into(),
                row_sha256: digest('2'),
            },
            characterization_profile: fingerprint_value('3'),
            characterization_id: "characterization-1".into(),
            characterization_sha256: digest('4'),
            effective_identity: EffectiveIdentityV1 {
                model: "model-1".into(),
                effort: Some("low".into()),
                mode: None,
            },
            caps: effect_caps(),
        };
        ProviderEffectGrantV1 {
            schema_version: 1,
            grant_id: "grant-1".into(),
            generation: 1,
            grant_sha256: digest('5'),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            host_identity_sha256: digest('6'),
            profile_policy_bundle_sha256: digest('7'),
            scheduler_binary_sha256: digest('8'),
            price_snapshot_sha256: digest('9'),
            price_snapshot_observed_at_ms: 1,
            price_snapshot_valid_until_ms: 200,
            legacy_inventory_sha256: digest('a'),
            triggers: vec![TriggerKindV1::Daily],
            case_ids: vec!["case-1".into()],
            provider_families: vec!["provider-1".into()],
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            per_run_caps: effect_caps(),
            budgets: GrantBudgetPolicyV1 {
                per_case: vec![NamedBudgetCapsV1 {
                    id: "case-1".into(),
                    caps: aggregate_caps(1),
                }],
                per_trigger_pool: vec![TriggerBudgetCapsV1 {
                    trigger: TriggerKindV1::Daily,
                    caps: aggregate_caps(1),
                }],
                per_provider: vec![NamedBudgetCapsV1 {
                    id: "provider-1".into(),
                    caps: aggregate_caps(1),
                }],
                utc_day: aggregate_caps(2),
                rolling_24h: aggregate_caps(2),
                protected_scheduled: aggregate_caps(1),
                protected_test_merge: aggregate_caps(1),
                manual_unallocated: aggregate_caps(1),
            },
            confirmation_allowance: 1,
            launchd: Vec::new(),
            profiles: vec![profile],
            not_before_ms: 1,
            expires_at_ms: 100,
            revocation_generation: 1,
        }
    }

    fn publication_outbox() -> PublicationOutboxV1 {
        PublicationOutboxV1 {
            schema_version: 1,
            outbox_id: "outbox-1".into(),
            state: PublicationOutboxStateV1::RemotePending,
            repository: "shoedog/a2acp".into(),
            pull_request: 37,
            test_merge_sha256: digest('a'),
            context: "a2a-bridge/r3d".into(),
            app_id: "app-1".into(),
            external_id: "external-1".into(),
            check_run: OptionalCheckRunIdV1::CheckRun { id: 1 },
            terminal_consumption: OptionalStableIdV1::Absent,
            desired_conclusion: OptionalCheckConclusionV1::Absent,
            evidence_set: OptionalSha256V1::Absent,
            final_guard: OptionalSha256V1::Absent,
            remote_observation: OptionalSha256V1::Absent,
            remote_observation_attempts: 0,
        }
    }

    fn execution_record(
        profile: FingerprintV1,
        source_sha256: String,
        row_sha256: String,
        identity: EffectiveIdentityV1,
        caps: EffectCapsV1,
    ) -> CaseExecutionFingerprintRecordV1 {
        let input = CaseExecutionFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: profile,
            target: ExactExecutionTargetV1::RepositorySnapshot {
                repository: "shoedog/a2acp".into(),
                head_sha256: digest('a'),
                tree_sha256: digest('b'),
                range_start_exclusive: OptionalSha256V1::Absent,
            },
            candidate: CandidateBinaryIdentityV1 {
                sha256: digest('c'),
                length_bytes: 1,
                build_provenance_sha256: digest('d'),
            },
            bindings: ExactExecutionBindingsV1 {
                source_sha256,
                row_sha256,
                run_manifest_sha256: digest('e'),
                generated_config_sha256: digest('f'),
                pin_set_sha256: digest('1'),
                resolution_bundle: OptionalSha256V1::Absent,
                package_integrity_sha256: digest('2'),
                image_digest: OptionalSha256V1::Absent,
                base_image_digest: OptionalSha256V1::Absent,
                environment_sha256: digest('3'),
                prerequisites_sha256: digest('4'),
            },
            requested_identity: identity.clone(),
            expected_effective_identity: identity,
            actual_caps: caps,
        };
        let sha256 = canonical_input_sha256("test execution", &input).unwrap();
        CaseExecutionFingerprintRecordV1 {
            schema_version: 1,
            input,
            fingerprint: FingerprintV1 {
                schema_version: 1,
                sha256,
            },
        }
    }

    fn admission_record(
        profile: FingerprintV1,
        execution: FingerprintV1,
        authority: AdmissionAuthorityV1,
        source: TriggerSourceV1,
        kind: TriggerKindV1,
    ) -> AdmissionAttemptFingerprintRecordV1 {
        let input = AdmissionAttemptFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: profile,
            case_execution: execution,
            authority,
            trigger: AdmissionTriggerIdentityV1 {
                source,
                kind,
                request_id: "request-1".into(),
                window_id: "window-1".into(),
                attempt_id: "attempt-1".into(),
                repeat_nonce: OptionalStableIdV1::Absent,
            },
        };
        let sha256 = canonical_input_sha256("test admission", &input).unwrap();
        AdmissionAttemptFingerprintRecordV1 {
            schema_version: 1,
            input,
            fingerprint: FingerprintV1 {
                schema_version: 1,
                sha256,
            },
        }
    }

    #[test]
    fn admission_authority_is_a_strict_tagged_union() {
        let valid = serde_json::to_value(standing_authority()).unwrap();
        serde_json::from_value::<AdmissionAuthorityV1>(valid).unwrap();

        let mixed = serde_json::json!({
            "kind": "standing_grant",
            "grant_id": "grant-1",
            "generation": 1,
            "grant_sha256": digest('a'),
            "characterization_id": "characterization-1",
            "characterization_sha256": digest('b'),
            "request_nonce": "manual-must-not-mix"
        });
        assert!(serde_json::from_value::<AdmissionAuthorityV1>(mixed).is_err());
        let unknown = serde_json::json!({"kind": "implicit"});
        assert!(serde_json::from_value::<AdmissionAuthorityV1>(unknown).is_err());
    }

    #[test]
    fn grant_budget_dimensions_require_exact_coverage_and_bounded_pools() {
        let case_ids = BTreeSet::from(["case-1"]);
        let provider_families = BTreeSet::from(["provider-1"]);
        let triggers = BTreeSet::from([TriggerKindV1::Daily]);
        let mut budgets = GrantBudgetPolicyV1 {
            per_case: vec![NamedBudgetCapsV1 {
                id: "case-1".into(),
                caps: aggregate_caps(1),
            }],
            per_trigger_pool: vec![TriggerBudgetCapsV1 {
                trigger: TriggerKindV1::Daily,
                caps: aggregate_caps(1),
            }],
            per_provider: vec![NamedBudgetCapsV1 {
                id: "provider-1".into(),
                caps: aggregate_caps(1),
            }],
            utc_day: aggregate_caps(2),
            rolling_24h: aggregate_caps(2),
            protected_scheduled: aggregate_caps(1),
            protected_test_merge: aggregate_caps(1),
            manual_unallocated: aggregate_caps(1),
        };
        validate_grant_budgets(&budgets, &case_ids, &provider_families, &triggers).unwrap();

        let provider = budgets.per_provider.pop().unwrap();
        assert!(
            validate_grant_budgets(&budgets, &case_ids, &provider_families, &triggers)
                .unwrap_err()
                .to_string()
                .contains("per-provider")
        );
        budgets.per_provider.push(provider);
        budgets.protected_test_merge.max_attempts = 3;
        assert!(
            validate_grant_budgets(&budgets, &case_ids, &provider_families, &triggers)
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
    }

    #[test]
    fn provider_grant_rejects_two_profiles_for_one_case() {
        let mut grant = provider_grant();
        grant.validate().unwrap();

        let mut duplicate_case = grant.profiles[0].clone();
        duplicate_case.characterization_profile = fingerprint_value('b');
        duplicate_case.characterization_id = "characterization-2".into();
        duplicate_case.characterization_sha256 = digest('c');
        grant.profiles.push(duplicate_case);

        assert!(grant
            .validate()
            .unwrap_err()
            .to_string()
            .contains("exactly one characterized profile"));
    }

    #[test]
    fn characterization_requires_one_shot_authority_and_matching_identity() {
        let identity = EffectiveIdentityV1 {
            model: "gpt-5.6-luna".into(),
            effort: Some("low".into()),
            mode: None,
        };
        let mut record = CharacterizationRecordV1 {
            schema_version: 1,
            characterization_id: "characterization-1".into(),
            source: ProfileSourceRefV1 {
                kind: ProfileSourceKindV1::ScheduledAdvisory,
                schema_version: 1,
                source_sha256: digest('a'),
                row_id: "case-1".into(),
                row_sha256: digest('b'),
            },
            profile_policy_bundle_sha256: digest('c'),
            characterization_profile: fingerprint_value('d'),
            case_execution: fingerprint_value('e'),
            admission_attempt: fingerprint_value('f'),
            authority: one_shot_authority(),
            expected_effective_identity: identity.clone(),
            observed_effective_identity: identity,
            outcome: CharacterizationOutcomeV1::CharacterizedGreen,
            evidence_sha256: digest('1'),
            terminal_at_ms: 1,
        };
        record.validate().unwrap();
        record.authority = standing_authority();
        assert!(record
            .validate()
            .unwrap_err()
            .to_string()
            .contains("one-shot"));

        record.authority = one_shot_authority();
        record.observed_effective_identity.model = "unexpected-model".into();
        assert!(record.validate().is_err());
        record.outcome = CharacterizationOutcomeV1::CharacterizationInconclusive;
        record.validate().unwrap();
    }

    #[test]
    fn manual_and_storage_records_remain_narrowly_scoped() {
        let mut manual = ManualAdmissionV1 {
            schema_version: 1,
            request_nonce: "request-1".into(),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            scheduler_binary_sha256: digest('a'),
            input_source_sha256: digest('b'),
            characterization_profile: fingerprint_value('c'),
            case_execution: fingerprint_value('d'),
            evidence_purpose: EvidencePurposeV1::ManualDiagnostic,
            freshness_bucket: "manual-1".into(),
            command: "compatibility run".into(),
            caps: effect_caps(),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            retry_cap: 0,
            fallback_cap: 0,
            acknowledged_billable: true,
            issued_at_ms: 1,
            expires_at_ms: 2,
        };
        manual.validate().unwrap();
        manual.command = "serve compatibility".into();
        assert!(manual.validate().is_err());

        let mut consent = StorageConsentV1 {
            schema_version: 1,
            consent_id: "consent-1".into(),
            consent_sha256: digest('e'),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            evidence_classes: vec![EvidenceClassV1::RoutineGreen],
            cold_root: "~/Documents/a2a-bridge/evidence-archive".into(),
            replication_mode: ReplicationModeV1::OwnerIcloud,
            file_provider_domain_id: "icloud-drive".into(),
            not_before_ms: 1,
            expires_at_ms: 2,
            revocation_generation: 1,
        };
        consent.validate().unwrap();
        consent.cold_root = "~/Documents/unapproved".into();
        assert!(consent.validate().is_err());

        consent.cold_root = "~/Documents/a2a-bridge/evidence-archive".into();
        let mut unknown_scope = serde_json::to_value(&consent).unwrap();
        unknown_scope["evidence_classes"] = serde_json::json!(["secret_dump"]);
        assert!(serde_json::from_value::<StorageConsentV1>(unknown_scope).is_err());
    }

    #[test]
    fn ledger_release_and_consumption_purpose_fail_closed() {
        let mut reconciliation = LedgerRecordV1::Reconciliation(LedgerReconciliationV1 {
            schema_version: 1,
            reservation_id: "reservation-1".into(),
            characterization_profile: fingerprint_value('a'),
            case_execution: fingerprint_value('b'),
            admission_attempt: fingerprint_value('c'),
            authority: standing_authority(),
            equivalent_work_key: digest('d'),
            terminal_evidence_sha256: digest('e'),
            disposition: LedgerDispositionV1::ReleasedPreEffect,
            charged_usage: UsageChargeV1 {
                attempts: 0,
                tokens: 0,
                cost_microusd: 0,
                elapsed_millis: 0,
            },
            prompt_may_have_been_accepted: false,
            reconciled_at_ms: 1,
        });
        reconciliation.validate().unwrap();
        let LedgerRecordV1::Reconciliation(value) = &mut reconciliation else {
            unreachable!()
        };
        value.charged_usage.attempts = 1;
        assert!(reconciliation.validate().is_err());

        let mut consumption = ConsumptionRecordV1 {
            schema_version: 1,
            consumption_id: "consumption-1".into(),
            equivalent_work_key: digest('f'),
            evidence_sha256: digest('1'),
            requested_purpose: EvidencePurposeV1::ProviderPathAdvisory,
            satisfied_purpose: EvidencePurposeV1::ClaimedSupportGate,
            characterization_profile: fingerprint_value('2'),
            case_execution: fingerprint_value('3'),
            admission_attempt: fingerprint_value('4'),
            authority: standing_authority(),
            consumed_at_ms: 1,
        };
        consumption.validate().unwrap();
        consumption.requested_purpose = EvidencePurposeV1::ManualDiagnostic;
        assert!(consumption.validate().is_err());
    }

    #[test]
    fn failure_disposition_keeps_unknown_out_of_confirmation_and_suppression() {
        let base = FailureDispositionV1 {
            schema_version: 1,
            characterization_profile: fingerprint_value('b'),
            case_execution: fingerprint_value('c'),
            evidence_sha256: digest('d'),
            failure_kind: FailureKindV1::CandidateUnknown,
            typed_code: "catalog.unavailable".into(),
            identical_complete_occurrences: 2,
            action: FailureActionV1::UnknownRetained,
            first_seen_ms: 1,
            last_seen_ms: 2,
        };
        base.validate().unwrap();
        let suppressed = FailureDispositionV1 {
            action: FailureActionV1::Suppressed,
            ..base
        };
        assert!(suppressed.validate().is_err());

        let first_untyped: FailureDispositionV1 = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "characterization_profile": fingerprint_value('1'),
            "case_execution": fingerprint_value('2'),
            "evidence_sha256": digest('3'),
            "failure_kind": "untyped_transient",
            "typed_code": "untyped.failure",
            "identical_complete_occurrences": 1,
            "action": "confirmation_due",
            "first_seen_ms": 1,
            "last_seen_ms": 1
        }))
        .unwrap();
        first_untyped.validate().unwrap();

        let second_untyped = FailureDispositionV1 {
            identical_complete_occurrences: 2,
            action: FailureActionV1::Suppressed,
            ..first_untyped
        };
        assert!(second_untyped.validate().is_err());
    }

    #[test]
    fn scheduled_source_rejects_manual_authority_and_one_shot_unattended_trigger() {
        let source = ProfileSourceRefV1 {
            kind: ProfileSourceKindV1::ScheduledAdvisory,
            schema_version: 1,
            source_sha256: digest('d'),
            row_id: "case-1".into(),
            row_sha256: digest('e'),
        };
        let profile = fingerprint_value('2');
        let identity = EffectiveIdentityV1 {
            model: "haiku".into(),
            effort: Some("low".into()),
            mode: None,
        };
        let caps = effect_caps();
        let case_execution = execution_record(
            profile.clone(),
            source.source_sha256.clone(),
            source.row_sha256.clone(),
            identity.clone(),
            caps.clone(),
        );
        let authority = standing_authority();
        let admission_attempt = admission_record(
            profile.clone(),
            case_execution.fingerprint.clone(),
            authority.clone(),
            TriggerSourceV1::DailyLaunchd,
            TriggerKindV1::Daily,
        );
        let mut record = ScheduledExecutionSourceV1 {
            schema_version: 1,
            source_sha256: digest('f'),
            source,
            profile_policy_bundle_sha256: digest('1'),
            characterization_profile: profile,
            case_execution,
            admission_attempt,
            authority,
            trigger: TriggerKindV1::Daily,
            requested_identity: identity.clone(),
            expected_effective_identity: identity,
            caps,
            retry_cap: 0,
            fallback_cap: 0,
        };
        record.validate().unwrap();
        let valid = record.clone();
        record.authority =
            AdmissionAuthorityV1::ManualAcknowledgement(ManualAcknowledgementAuthorityV1 {
                manual_admission_sha256: digest('8'),
                request_nonce: "request-1".into(),
            });
        assert!(record.validate().is_err());

        let mut drift = valid.clone();
        drift.requested_identity.model = "different-model".into();
        assert!(drift
            .validate()
            .unwrap_err()
            .to_string()
            .contains("inconsistent canonical bindings"));

        let mut unattended_one_shot = valid;
        let authority = one_shot_authority();
        unattended_one_shot.authority = authority.clone();
        unattended_one_shot.admission_attempt = admission_record(
            unattended_one_shot.characterization_profile.clone(),
            unattended_one_shot.case_execution.fingerprint.clone(),
            authority,
            TriggerSourceV1::DailyLaunchd,
            TriggerKindV1::Daily,
        );
        assert!(unattended_one_shot.validate().is_err());
    }

    #[test]
    fn publication_outbox_requires_terminal_fields_only_from_prepared_onward() {
        let mut outbox = publication_outbox();
        outbox.validate().unwrap();
        outbox.state = PublicationOutboxStateV1::Prepared;
        assert!(outbox.validate().is_err());
        outbox.terminal_consumption = OptionalStableIdV1::StableId {
            value: "consumption-1".into(),
        };
        outbox.desired_conclusion = OptionalCheckConclusionV1::Conclusion {
            value: CheckConclusionV1::Success,
        };
        outbox.evidence_set = OptionalSha256V1::Sha256 { value: digest('b') };
        outbox.final_guard = OptionalSha256V1::Sha256 { value: digest('c') };
        outbox.validate().unwrap();

        outbox.state = PublicationOutboxStateV1::RemotelyObserved;
        outbox.remote_observation = OptionalSha256V1::Sha256 { value: digest('d') };
        assert!(outbox.validate().is_err());
        outbox.remote_observation_attempts = 1;
        outbox.validate().unwrap();
    }

    #[test]
    fn publication_outbox_rejects_null_absence_and_partial_preterminal_state() {
        let ambiguous = serde_json::from_value::<PublicationOutboxV1>(serde_json::json!({
            "schema_version": 1,
            "outbox_id": "outbox-1",
            "state": "create_intent",
            "repository": "shoedog/a2acp",
            "pull_request": 37,
            "test_merge_sha256": digest('a'),
            "context": "a2a-bridge/r3d",
            "app_id": "app-1",
            "external_id": "external-1",
            "check_run_id": null,
            "terminal_consumption_id": null,
            "desired_conclusion": null,
            "evidence_set_sha256": null,
            "final_guard_sha256": null,
            "remote_observation_sha256": null
        }));
        assert!(ambiguous.is_err());

        let mut partial = publication_outbox();
        partial.terminal_consumption = OptionalStableIdV1::StableId {
            value: "consumption-1".into(),
        };
        assert!(partial.validate().is_err());
    }

    #[test]
    fn record_parser_rejects_unknown_fields_before_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "case_execution": fingerprint_value('a'),
                "failure_kind": "candidate_unknown",
                "typed_code": "catalog.unavailable",
                "identical_complete_occurrences": 1,
                "action": "unknown_retained",
                "first_seen_ms": 1,
                "last_seen_ms": 1,
                "caller_prompt": "not allowed"
            }))
            .unwrap(),
        )
        .unwrap();
        let error = validate_schedule_record("failure-disposition", &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown field"), "{error}");
    }
}
