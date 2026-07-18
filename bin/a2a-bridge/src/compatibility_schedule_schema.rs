//! Strict R3d0 record schemas used by later scheduling slices.
//!
//! These DTOs and validators are inert contracts. Parsing them performs no authority mutation,
//! credential access, provider call, registry/image operation, or GitHub publication.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::compatibility_schedule::{
    EffectCapsV1, EffectClassV1, EvidencePurposeV1, ReplicationModeV1, TriggerKindV1,
    EXPECTED_SUPPORT_PROFILES,
};
use crate::{compatibility, local_file, BoxError};

const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_ITEMS: usize = 256;
const MAX_CANDIDATE_BYTES: u64 = 256 * 1024 * 1024;
const CLAIMED_SUPPORT_READER_CASE_IDS: [&str; 2] =
    ["claude-reader-055-fable", "codex-reader-bridge-gpt56-sol"];

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
    pub(super) effort: OptionalTextV1,
    pub(super) mode: OptionalTextV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalTextV1 {
    Absent,
    Text { value: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum GitObjectAlgorithmV1 {
    Sha1,
    Sha256,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GitObjectIdV1 {
    pub(super) algorithm: GitObjectAlgorithmV1,
    pub(super) hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalGitObjectIdV1 {
    Absent,
    ObjectId { value: GitObjectIdV1 },
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
        head_oid: GitObjectIdV1,
        tree_oid: GitObjectIdV1,
        range_start_exclusive: OptionalGitObjectIdV1,
    },
    TestMerge {
        repository: String,
        pull_request: u64,
        base_oid: GitObjectIdV1,
        head_oid: GitObjectIdV1,
        merge_oid: GitObjectIdV1,
        merge_ref: String,
        tree_oid: GitObjectIdV1,
        ordered_parents: Vec<GitObjectIdV1>,
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
    pub(super) source: ManualAdmissionSourceV1,
    pub(super) command: ManualCommandV1,
    pub(super) caps: EffectCapsV1,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) retry_cap: u8,
    pub(super) fallback_cap: u8,
    pub(super) acknowledged_billable: bool,
    pub(super) issued_at_ms: i64,
    pub(super) expires_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ManualAdmissionSourceV1 {
    DirectLocalCli,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ManualCommandV1 {
    CompatibilityRun,
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
    pub(super) lifecycle: HoldLifecycleV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum HoldLifecycleV1 {
    Active,
    Cleared {
        opening_sha256: String,
        clearance_action_id: String,
        clearance_action_sha256: String,
        cleared_at_ms: i64,
        operator: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum QuarantineV1 {
    Open {
        schema_version: u16,
        quarantine_id: String,
        profile: FingerprintV1,
        operator: String,
        reason: String,
        created_at_ms: i64,
        expires_at_ms: i64,
    },
    Closed {
        schema_version: u16,
        quarantine_id: String,
        profile: FingerprintV1,
        opening_sha256: String,
        operator: String,
        reason: String,
        created_at_ms: i64,
        closed_at_ms: i64,
    },
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
    pub(super) base_oid: GitObjectIdV1,
    pub(super) head_oid: GitObjectIdV1,
    pub(super) merge_oid: GitObjectIdV1,
    pub(super) merge_ref: String,
    pub(super) tree_oid: GitObjectIdV1,
    pub(super) ordered_parents: Vec<GitObjectIdV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ImpactRecordV1 {
    pub(super) schema_version: u16,
    pub(super) classifier_sha256: String,
    pub(super) target: TestMergeIdentityV1,
    pub(super) classes: Vec<ImpactClassV1>,
    pub(super) due_case_ids: Vec<String>,
    pub(super) characterization_required_case_ids: Vec<String>,
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
    pub(super) provenance: ConsumptionEvidenceProvenanceV1,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) case_execution: FingerprintV1,
    pub(super) admission_attempt: FingerprintV1,
    pub(super) authority: AdmissionAuthorityV1,
    pub(super) consumed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ConsumptionEvidenceProvenanceV1 {
    Ordinary,
    ReviewedCharacterization {
        characterization_id: String,
        characterization_record_sha256: String,
        freshness_bucket: String,
        freshness_observation_sha256: String,
        terminal_at_ms: i64,
        reviewed_at_ms: i64,
        reviewer: String,
    },
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
    Identity { value: EffectiveIdentityV1 },
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
    pub(super) immutable_identity: FingerprintV1,
    pub(super) previous_record: OptionalSha256V1,
    pub(super) state: PublicationOutboxStateV1,
    pub(super) repository: String,
    pub(super) pull_request: u64,
    pub(super) test_merge_oid: GitObjectIdV1,
    pub(super) context: String,
    pub(super) app_id: String,
    pub(super) external_id: String,
    pub(super) check_run: OptionalCheckRunIdV1,
    pub(super) check_run_binding: OptionalSha256V1,
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
    pub(super) hot_path: RelativeEvidencePathV1,
    pub(super) cold_path: OptionalRelativeEvidencePathV1,
    pub(super) full_retain_until_ms: i64,
    pub(super) compact_retain_until_ms: i64,
    pub(super) pinned: bool,
    pub(super) lease_count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RelativeEvidencePathV1 {
    pub(super) components: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalRelativeEvidencePathV1 {
    Absent,
    RelativePath { value: RelativeEvidencePathV1 },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ColdStorageBindingV1 {
    Absent,
    OwnerIcloud {
        consent_id: String,
        consent_sha256: String,
        root_sha256: String,
        file_provider_domain_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceIndexV1 {
    pub(super) schema_version: u16,
    pub(super) index_id: String,
    pub(super) generation: u64,
    pub(super) hot_root_sha256: String,
    pub(super) cold_storage: ColdStorageBindingV1,
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
    pub(super) last_outcome: OptionalTextV1,
    pub(super) hold: OptionalRecordRefV1,
    pub(super) quarantine: OptionalRecordRefV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalWindowV1 {
    Absent,
    Window { id: String, scheduled_at_ms: i64 },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AuthorityStateV1 {
    Active,
    Blocked,
    Expired,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalAuthorityStatusV1 {
    Absent,
    Authority {
        id: String,
        sha256: String,
        state: AuthorityStateV1,
        expires_at_ms: i64,
        revocation_generation: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum StorageStateV1 {
    HotOnly,
    ColdEligible,
    Archiving,
    Synchronized,
    Blocked,
    QuotaPressure,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum OneShotCompatibilityStateV1 {
    Pass,
    Fail,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum SharedOperatorHealthV1 {
    NotEvaluated,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ScheduleStatusV1 {
    pub(super) schema_version: u16,
    pub(super) generated_at_ms: i64,
    pub(super) policy_sha256: String,
    pub(super) last_window: OptionalWindowV1,
    pub(super) next_window: OptionalWindowV1,
    pub(super) provider_grant: OptionalAuthorityStatusV1,
    pub(super) storage_consent: OptionalAuthorityStatusV1,
    pub(super) ledger_headroom_sha256: String,
    pub(super) storage_state: StorageStateV1,
    pub(super) missed_ticks: u64,
    pub(super) fresh_one_shot_compatibility: OneShotCompatibilityStateV1,
    pub(super) shared_operator_health: SharedOperatorHealthV1,
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
    pub(super) primary_model_class: DogfoodPrimaryModelClassV1,
    pub(super) effort: DogfoodEffortPolicyV1,
    pub(super) second_opinion: DogfoodSecondOpinionV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum DogfoodPrimaryModelClassV1 {
    InexpensiveEligible,
    LunaOrSonnet,
    Sol,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum DogfoodEffortPolicyV1 {
    LowOrMedium,
    MediumOrHigh,
    High,
    HighOrXhigh,
    Xhigh,
    Max,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum DogfoodSecondOpinionV1 {
    SolWhenScopeExpands,
    SolForCrossCuttingDifficult,
    OpusIndependentArchitecture,
    OpusAssumptionsAlternativesGapsCrossCutting,
    OpusIndependentNoNestedHelpers,
    OpusXhighThenFableHardComplex,
    OpusOrFableAfterSolGreen,
    OpusXhighThenFableRiskJustified,
    OpusAlternativesThenFableHardComplex,
    FableAdversarialWhenUseful,
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

fn optional_text(label: &str, value: &OptionalTextV1) -> Result<(), BoxError> {
    if let OptionalTextV1::Text { value } = value {
        bounded_text(label, value)?;
    }
    Ok(())
}

fn git_oid(label: &str, value: &GitObjectIdV1) -> Result<(), BoxError> {
    let expected_len = match value.algorithm {
        GitObjectAlgorithmV1::Sha1 => 40,
        GitObjectAlgorithmV1::Sha256 => 64,
    };
    if value.hex.len() != expected_len
        || !value
            .hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || value.hex.bytes().all(|byte| byte == b'0')
    {
        return Err(format!(
            "schedule schema: {label} must be a lowercase {}-character {:?} Git object id",
            expected_len, value.algorithm
        )
        .into());
    }
    Ok(())
}

fn git_oid_set<'a>(
    label: &str,
    values: impl IntoIterator<Item = &'a GitObjectIdV1>,
) -> Result<(), BoxError> {
    let mut algorithm = None;
    for value in values {
        git_oid(label, value)?;
        if algorithm
            .replace(value.algorithm)
            .is_some_and(|seen| seen != value.algorithm)
        {
            return Err(format!(
                "schedule schema: {label} mixes Git object algorithms within one repository target"
            )
            .into());
        }
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
    if let OptionalEffectiveIdentityV1::Identity { value } = value {
        effective_identity(label, value)?;
    }
    Ok(())
}

fn effective_identity(label: &str, value: &EffectiveIdentityV1) -> Result<(), BoxError> {
    bounded_text(&format!("{label}.model"), &value.model)?;
    optional_text(&format!("{label}.effort"), &value.effort)?;
    optional_text(&format!("{label}.mode"), &value.mode)?;
    Ok(())
}

fn canonical_input_sha256<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| format!("schedule schema: cannot canonicalize {label}: {error}"))?;
    let mut domain_separated = format!("a2a-bridge:r3d0:{label}:v1\0").into_bytes();
    domain_separated.extend_from_slice(&canonical);
    Ok(local_file::sha256_hex(&domain_separated))
}

#[derive(Serialize)]
struct SafetyHoldOpeningIdentityV1<'a> {
    schema_version: u16,
    hold_id: &'a str,
    characterization_profile: &'a FingerprintV1,
    case_execution: &'a FingerprintV1,
    reason: HoldReasonV1,
    created_at_ms: i64,
}

fn safety_hold_opening_sha256(value: &SafetyHoldV1) -> Result<String, BoxError> {
    canonical_input_sha256(
        "safety-hold opening",
        &SafetyHoldOpeningIdentityV1 {
            schema_version: value.schema_version,
            hold_id: &value.hold_id,
            characterization_profile: &value.characterization_profile,
            case_execution: &value.case_execution,
            reason: value.reason,
            created_at_ms: value.created_at_ms,
        },
    )
}

#[derive(Serialize)]
struct SafetyHoldClearanceIdentityV1<'a> {
    schema_version: u16,
    opening_sha256: &'a str,
    clearance_action_id: &'a str,
    cleared_at_ms: i64,
    operator: &'a str,
    reason: &'a str,
}

#[derive(Serialize)]
struct PublicationOutboxIdentityInputV1<'a> {
    schema_version: u16,
    repository: &'a str,
    pull_request: u64,
    test_merge_oid: &'a GitObjectIdV1,
    context: &'a str,
    app_id: &'a str,
    external_id: &'a str,
}

fn publication_outbox_identity_sha256(value: &PublicationOutboxV1) -> Result<String, BoxError> {
    canonical_input_sha256(
        "publication-outbox immutable identity",
        &PublicationOutboxIdentityInputV1 {
            schema_version: value.schema_version,
            repository: &value.repository,
            pull_request: value.pull_request,
            test_merge_oid: &value.test_merge_oid,
            context: &value.context,
            app_id: &value.app_id,
            external_id: &value.external_id,
        },
    )
}

#[derive(Serialize)]
struct PublicationCheckRunBindingInputV1<'a> {
    immutable_identity: &'a FingerprintV1,
    check_run_id: u64,
}

fn publication_check_run_binding_sha256(
    immutable_identity: &FingerprintV1,
    check_run_id: u64,
) -> Result<String, BoxError> {
    canonical_input_sha256(
        "publication-outbox check-run binding",
        &PublicationCheckRunBindingInputV1 {
            immutable_identity,
            check_run_id,
        },
    )
}

fn validate_execution_target(target: &ExactExecutionTargetV1) -> Result<(), BoxError> {
    match target {
        ExactExecutionTargetV1::RepositorySnapshot {
            repository,
            head_oid,
            tree_oid,
            range_start_exclusive,
        } => {
            bounded_text("execution repository", repository)?;
            let mut object_ids = vec![head_oid, tree_oid];
            if let OptionalGitObjectIdV1::ObjectId { value } = range_start_exclusive {
                object_ids.push(value);
            }
            git_oid_set("repository-snapshot object ids", object_ids)
        }
        ExactExecutionTargetV1::TestMerge {
            repository,
            pull_request,
            base_oid,
            head_oid,
            merge_oid,
            merge_ref,
            tree_oid,
            ordered_parents,
        } => {
            bounded_text("test-merge repository", repository)?;
            git_oid_set(
                "test-merge object ids",
                [base_oid, head_oid, merge_oid, tree_oid]
                    .into_iter()
                    .chain(ordered_parents.iter()),
            )?;
            if *pull_request == 0
                || merge_ref != &format!("refs/pull/{pull_request}/merge")
                || ordered_parents != &[base_oid.clone(), head_oid.clone()]
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
    let sum = |field: &str, values: [u64; 3]| -> Result<u64, BoxError> {
        values
            .into_iter()
            .try_fold(0_u64, u64::checked_add)
            .ok_or_else(|| format!("schedule schema: {field} pool allocation overflows").into())
    };
    let allocated = AggregateBudgetCapsV1 {
        max_attempts: sum(
            "attempt",
            [
                value.protected_scheduled.max_attempts,
                value.protected_test_merge.max_attempts,
                value.manual_unallocated.max_attempts,
            ],
        )?,
        max_tokens: sum(
            "token",
            [
                value.protected_scheduled.max_tokens,
                value.protected_test_merge.max_tokens,
                value.manual_unallocated.max_tokens,
            ],
        )?,
        max_cost_microusd: sum(
            "cost",
            [
                value.protected_scheduled.max_cost_microusd,
                value.protected_test_merge.max_cost_microusd,
                value.manual_unallocated.max_cost_microusd,
            ],
        )?,
        max_time_secs: sum(
            "time",
            [
                value.protected_scheduled.max_time_secs,
                value.protected_test_merge.max_time_secs,
                value.manual_unallocated.max_time_secs,
            ],
        )?,
    };
    aggregate_budget_within(
        "combined protected/manual allocation",
        &allocated,
        &value.utc_day,
    )?;
    aggregate_budget_within(
        "combined protected/manual rolling allocation",
        &allocated,
        &value.rolling_24h,
    )?;

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
        let required_launchd = self
            .triggers
            .iter()
            .filter(|trigger| matches!(trigger, TriggerKindV1::Daily | TriggerKindV1::TestMerge))
            .copied()
            .collect::<BTreeSet<_>>();
        let observed_launchd = self
            .launchd
            .iter()
            .map(|item| item.trigger)
            .collect::<BTreeSet<_>>();
        if observed_launchd != required_launchd || observed_launchd.len() != self.launchd.len() {
            return Err(
                "schedule schema: daily and test-merge grant triggers require exactly one launchd label/plist binding"
                    .into(),
            );
        }
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
            if profile.source.row_id != profile.case_id {
                return Err(
                    "schedule schema: granted case id must match its characterized source row"
                        .into(),
                );
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
        if self.evidence_purpose == EvidencePurposeV1::Characterization {
            return Err(
                "schedule schema: generic manual admission cannot authorize characterization"
                    .into(),
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
        match &self.lifecycle {
            HoldLifecycleV1::Active => Ok(()),
            HoldLifecycleV1::Cleared {
                opening_sha256,
                clearance_action_id,
                clearance_action_sha256,
                cleared_at_ms,
                operator,
                reason,
            } if *cleared_at_ms >= self.created_at_ms => {
                sha256("hold opening", opening_sha256)?;
                stable_id("hold clearance action id", clearance_action_id)?;
                sha256("hold clearance action", clearance_action_sha256)?;
                bounded_text("hold clearance operator", operator)?;
                bounded_text("hold clearance reason", reason)?;
                let expected_opening = safety_hold_opening_sha256(self)?;
                if opening_sha256 != &expected_opening {
                    return Err(
                        "schedule schema: hold clearance does not bind the canonical opening record"
                            .into(),
                    );
                }
                let expected_clearance = canonical_input_sha256(
                    "safety-hold clearance action",
                    &SafetyHoldClearanceIdentityV1 {
                        schema_version: 1,
                        opening_sha256,
                        clearance_action_id,
                        cleared_at_ms: *cleared_at_ms,
                        operator,
                        reason,
                    },
                )?;
                if clearance_action_sha256 != &expected_clearance {
                    return Err(
                        "schedule schema: hold clearance action fingerprint does not match".into(),
                    );
                }
                Ok(())
            }
            HoldLifecycleV1::Cleared { .. } => {
                Err("schedule schema: hold clearance predates the hold".into())
            }
        }
    }
}

impl ValidateRecord for QuarantineV1 {
    fn validate(&self) -> Result<(), BoxError> {
        match self {
            QuarantineV1::Open {
                schema_version,
                quarantine_id,
                profile,
                operator,
                reason,
                created_at_ms,
                expires_at_ms,
            } => {
                if *schema_version != 1 {
                    return Err("schedule schema: quarantine schema_version must be 1".into());
                }
                stable_id("quarantine id", quarantine_id)?;
                fingerprint("quarantined profile", profile)?;
                bounded_text("quarantine operator", operator)?;
                bounded_text("quarantine reason", reason)?;
                time_range("quarantine", *created_at_ms, *expires_at_ms)
            }
            QuarantineV1::Closed {
                schema_version,
                quarantine_id,
                profile,
                opening_sha256,
                operator,
                reason,
                created_at_ms,
                closed_at_ms,
            } => {
                if *schema_version != 1 || *created_at_ms <= 0 || *closed_at_ms < *created_at_ms {
                    return Err(
                        "schedule schema: quarantine closure has an invalid version or time range"
                            .into(),
                    );
                }
                stable_id("quarantine id", quarantine_id)?;
                fingerprint("quarantined profile", profile)?;
                sha256("quarantine opening", opening_sha256)?;
                bounded_text("quarantine closure operator", operator)?;
                bounded_text("quarantine closure reason", reason)
            }
        }
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
                FailureKindV1::UntypedTransient,
                2..,
                FailureActionV1::UnknownRetained
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
    git_oid_set(
        "test-merge object ids",
        [
            &target.base_oid,
            &target.head_oid,
            &target.merge_oid,
            &target.tree_oid,
        ]
        .into_iter()
        .chain(target.ordered_parents.iter()),
    )?;
    if target.ordered_parents != [target.base_oid.clone(), target.head_oid.clone()] {
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
        unique_ids(
            "characterization-required case id",
            self.characterization_required_case_ids
                .iter()
                .map(String::as_str),
        )?;
        let classes = self.classes.iter().copied().collect::<BTreeSet<_>>();
        let production = classes.iter().any(|class| {
            matches!(
                class,
                ImpactClassV1::AcpRuntime
                    | ImpactClassV1::ContainerRuntime
                    | ImpactClassV1::ModelCapability
                    | ImpactClassV1::Authentication
                    | ImpactClassV1::CompatibilityCore
            )
        });
        if classes.contains(&ImpactClassV1::DocumentationOnly) && classes.len() != 1 {
            return Err(
                "schedule schema: documentation_only is exclusive and cannot make provider cases due"
                    .into(),
            );
        }
        if classes == BTreeSet::from([ImpactClassV1::DocumentationOnly])
            || classes == BTreeSet::from([ImpactClassV1::TestsOnly])
        {
            if !self.due_case_ids.is_empty()
                || !self.characterization_required_case_ids.is_empty()
                || !self.no_impact_proved
            {
                return Err(
                    "schedule schema: docs-only/tests-only impact must prove no provider impact"
                        .into(),
                );
            }
            return Ok(());
        }
        if production == self.due_case_ids.is_empty() {
            return Err(
                "schedule schema: production impact classes and due provider cases contradict"
                    .into(),
            );
        }
        if classes.contains(&ImpactClassV1::NewProvider)
            == self.characterization_required_case_ids.is_empty()
        {
            return Err(
                "schedule schema: new-provider impact must be characterization-required, never immediately due"
                .into(),
            );
        }
        let due = self
            .due_case_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let claimed_support = EXPECTED_SUPPORT_PROFILES
            .into_iter()
            .collect::<BTreeSet<_>>();
        let readers = CLAIMED_SUPPORT_READER_CASE_IDS
            .into_iter()
            .collect::<BTreeSet<_>>();
        if !due.is_subset(&claimed_support) {
            return Err(
                "schedule schema: due cases must be exact inventoried claimed-support profiles"
                    .into(),
            );
        }
        let requires_all = classes.contains(&ImpactClassV1::AcpRuntime)
            || classes.contains(&ImpactClassV1::CompatibilityCore);
        if requires_all && !claimed_support.is_subset(&due) {
            return Err(
                "schedule schema: ACP/core impact must make every claimed-support profile due"
                    .into(),
            );
        }
        if classes.contains(&ImpactClassV1::ContainerRuntime) && !readers.is_subset(&due) {
            return Err(
                "schedule schema: container impact must make both claimed-support reader profiles due"
                    .into(),
            );
        }
        let broad_case_class = classes.contains(&ImpactClassV1::AcpRuntime)
            || classes.contains(&ImpactClassV1::CompatibilityCore)
            || classes.contains(&ImpactClassV1::ModelCapability)
            || classes.contains(&ImpactClassV1::Authentication);
        if !broad_case_class && !due.is_subset(&readers) {
            return Err(
                "schedule schema: container-only impact cannot make host profiles due".into(),
            );
        }
        if due.iter().any(|case| {
            self.characterization_required_case_ids
                .iter()
                .any(|id| id == case)
        }) {
            return Err(
                "schedule schema: one case cannot be both due and characterization-required".into(),
            );
        }
        if self.no_impact_proved || (!production && !classes.contains(&ImpactClassV1::NewProvider))
        {
            return Err("schedule schema: impact reducer state is contradictory".into());
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
        let purpose_allowed = match &self.provenance {
            ConsumptionEvidenceProvenanceV1::Ordinary => {
                self.requested_purpose == self.satisfied_purpose
                    || (self.requested_purpose == EvidencePurposeV1::ProviderPathAdvisory
                        && self.satisfied_purpose == EvidencePurposeV1::ClaimedSupportGate)
            }
            ConsumptionEvidenceProvenanceV1::ReviewedCharacterization {
                characterization_id,
                characterization_record_sha256,
                freshness_bucket,
                freshness_observation_sha256,
                terminal_at_ms,
                reviewed_at_ms,
                reviewer,
            } => {
                stable_id("consumed characterization id", characterization_id)?;
                sha256(
                    "consumed characterization record",
                    characterization_record_sha256,
                )?;
                stable_id(
                    "consumed characterization freshness bucket",
                    freshness_bucket,
                )?;
                sha256(
                    "consumed characterization freshness observation",
                    freshness_observation_sha256,
                )?;
                bounded_text("consumed characterization reviewer", reviewer)?;
                if *terminal_at_ms <= 0
                    || *reviewed_at_ms < *terminal_at_ms
                    || self.consumed_at_ms < *reviewed_at_ms
                {
                    return Err(
                        "schedule schema: reviewed characterization provenance times are invalid"
                            .into(),
                    );
                }
                self.requested_purpose == EvidencePurposeV1::ProviderPathAdvisory
                    && self.satisfied_purpose == EvidencePurposeV1::Characterization
            }
        };
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
        fingerprint("outbox immutable identity", &self.immutable_identity)?;
        optional_sha256("outbox previous record", &self.previous_record)?;
        bounded_text("outbox repository", &self.repository)?;
        git_oid("outbox test merge", &self.test_merge_oid)?;
        bounded_text("check context", &self.context)?;
        stable_id("App id", &self.app_id)?;
        stable_id("external id", &self.external_id)?;
        let expected_identity = publication_outbox_identity_sha256(self)?;
        if self.immutable_identity.sha256 != expected_identity
            || self.outbox_id != format!("outbox:{expected_identity}")
        {
            return Err(
                "schedule schema: outbox id does not bind its canonical immutable remote identity"
                    .into(),
            );
        }
        let has_previous = matches!(self.previous_record, OptionalSha256V1::Sha256 { .. });
        if (self.state == PublicationOutboxStateV1::CreateIntent) == has_previous {
            return Err(
                "schedule schema: only create-intent may omit the previous outbox record hash"
                    .into(),
            );
        }
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
        let check_run_id = match &self.check_run {
            OptionalCheckRunIdV1::Absent => None,
            OptionalCheckRunIdV1::CheckRun { id } if *id > 0 => Some(*id),
            OptionalCheckRunIdV1::CheckRun { .. } => {
                return Err("schedule schema: check_run id must be positive".into())
            }
        };
        let check_run_bound = check_run_id.is_some();
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
        match (check_run_id, &self.check_run_binding) {
            (None, OptionalSha256V1::Absent) => {}
            (Some(id), OptionalSha256V1::Sha256 { value }) => {
                sha256("outbox check-run binding", value)?;
                let expected = publication_check_run_binding_sha256(&self.immutable_identity, id)?;
                if value != &expected {
                    return Err(
                        "schedule schema: check-run id is not bound to the immutable outbox identity"
                            .into(),
                    );
                }
            }
            _ => {
                return Err(
                    "schedule schema: check-run binding presence disagrees with the remote id"
                        .into(),
                )
            }
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

fn portable_path_component(label: &str, value: &str) -> Result<(), BoxError> {
    bounded_text(label, value)?;
    let invalid_character = value.bytes().any(|byte| {
        matches!(
            byte,
            b'/' | b'\\' | b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|'
        )
    });
    let stem = value
        .split_once('.')
        .map_or(value, |(stem, _)| stem)
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if !value.is_ascii()
        || value == "."
        || value == ".."
        || value.ends_with([' ', '.'])
        || invalid_character
        || reserved
    {
        return Err(format!(
            "schedule schema: {label} must be one normalized portable relative-path component"
        )
        .into());
    }
    Ok(())
}

fn portable_evidence_path_key(value: &RelativeEvidencePathV1) -> String {
    value
        .components
        .iter()
        .map(|component| component.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

fn relative_evidence_path(label: &str, value: &RelativeEvidencePathV1) -> Result<(), BoxError> {
    if value.components.is_empty() || value.components.len() > 64 {
        return Err(format!(
            "schedule schema: {label} must contain 1..=64 relative path components"
        )
        .into());
    }
    for component in &value.components {
        portable_path_component(label, component)?;
    }
    Ok(())
}

impl ValidateRecord for EvidenceIndexV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.generation == 0 || self.entries.len() > MAX_ITEMS {
            return Err(
                "schedule schema: evidence index must be version 1 with a positive generation and bounded entries"
                    .into(),
            );
        }
        stable_id("evidence index id", &self.index_id)?;
        sha256("hot evidence root", &self.hot_root_sha256)?;
        let cold_enabled = match &self.cold_storage {
            ColdStorageBindingV1::Absent => false,
            ColdStorageBindingV1::OwnerIcloud {
                consent_id,
                consent_sha256,
                root_sha256,
                file_provider_domain_id,
            } => {
                stable_id("cold storage consent id", consent_id)?;
                sha256("cold storage consent", consent_sha256)?;
                sha256("cold storage root", root_sha256)?;
                bounded_text("cold storage FileProvider domain", file_provider_domain_id)?;
                true
            }
        };
        unique_ids(
            "evidence id",
            self.entries.iter().map(|entry| entry.evidence_id.as_str()),
        )?;
        let mut hot_paths = BTreeSet::new();
        let mut cold_paths = BTreeSet::new();
        for entry in &self.entries {
            sha256("full evidence", &entry.full_evidence_sha256)?;
            sha256("compact evidence", &entry.compact_record_sha256)?;
            relative_evidence_path("hot evidence path", &entry.hot_path)?;
            if !hot_paths.insert(portable_evidence_path_key(&entry.hot_path)) {
                return Err(
                    "schedule schema: hot evidence paths must be unique in the portable namespace"
                        .into(),
                );
            }
            match &entry.cold_path {
                OptionalRelativeEvidencePathV1::Absent => {}
                OptionalRelativeEvidencePathV1::RelativePath { value } if cold_enabled => {
                    relative_evidence_path("cold evidence path", value)?;
                    if !cold_paths.insert(portable_evidence_path_key(value)) {
                        return Err("schedule schema: cold evidence paths must be unique in the portable namespace".into());
                    }
                }
                OptionalRelativeEvidencePathV1::RelativePath { .. } => {
                    return Err(
                        "schedule schema: cold evidence path requires a bound cold-storage root"
                            .into(),
                    )
                }
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

fn optional_window(label: &str, value: &OptionalWindowV1) -> Result<(), BoxError> {
    if let OptionalWindowV1::Window {
        id,
        scheduled_at_ms,
    } = value
    {
        stable_id(&format!("{label} id"), id)?;
        if *scheduled_at_ms <= 0 {
            return Err(format!("schedule schema: {label} time must be positive").into());
        }
    }
    Ok(())
}

fn optional_authority_status(
    label: &str,
    value: &OptionalAuthorityStatusV1,
    generated_at_ms: i64,
) -> Result<(), BoxError> {
    if let OptionalAuthorityStatusV1::Authority {
        id,
        sha256: value,
        state,
        expires_at_ms,
        revocation_generation,
    } = value
    {
        stable_id(&format!("{label} id"), id)?;
        sha256(label, value)?;
        if *expires_at_ms <= 0 || *revocation_generation == 0 {
            return Err(
                format!("schedule schema: {label} expiry/generation state is invalid").into(),
            );
        }
        let expired = *expires_at_ms <= generated_at_ms;
        if (*state == AuthorityStateV1::Active && expired)
            || (*state == AuthorityStateV1::Expired && !expired)
        {
            return Err(format!(
                "schedule schema: {label} expiry time and authority state disagree"
            )
            .into());
        }
    }
    Ok(())
}

impl ValidateRecord for ScheduleStatusV1 {
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.generated_at_ms <= 0 || self.cases.len() > MAX_ITEMS {
            return Err(
                "schedule schema: status must be version 1, timestamped, and bounded".into(),
            );
        }
        sha256("status policy", &self.policy_sha256)?;
        optional_window("last window", &self.last_window)?;
        optional_window("next window", &self.next_window)?;
        if let OptionalWindowV1::Window {
            scheduled_at_ms, ..
        } = &self.last_window
        {
            if *scheduled_at_ms > self.generated_at_ms {
                return Err(
                    "schedule schema: last window cannot follow status generation time".into(),
                );
            }
        }
        if let OptionalWindowV1::Window {
            scheduled_at_ms, ..
        } = &self.next_window
        {
            if *scheduled_at_ms <= self.generated_at_ms {
                return Err(
                    "schedule schema: next window must follow status generation time".into(),
                );
            }
        }
        if let (
            OptionalWindowV1::Window {
                scheduled_at_ms: last,
                ..
            },
            OptionalWindowV1::Window {
                scheduled_at_ms: next,
                ..
            },
        ) = (&self.last_window, &self.next_window)
        {
            if next <= last {
                return Err("schedule schema: next window must follow the last window".into());
            }
        }
        optional_authority_status("provider grant", &self.provider_grant, self.generated_at_ms)?;
        optional_authority_status(
            "storage consent",
            &self.storage_consent,
            self.generated_at_ms,
        )?;
        let provider_grant_active = matches!(
            &self.provider_grant,
            OptionalAuthorityStatusV1::Authority {
                state: AuthorityStateV1::Active,
                expires_at_ms,
                ..
            } if *expires_at_ms > self.generated_at_ms
        );
        let storage_consent_present = matches!(
            &self.storage_consent,
            OptionalAuthorityStatusV1::Authority { .. }
        );
        let storage_consent_active = matches!(
            &self.storage_consent,
            OptionalAuthorityStatusV1::Authority {
                state: AuthorityStateV1::Active,
                expires_at_ms,
                ..
            } if *expires_at_ms > self.generated_at_ms
        );
        if matches!(
            self.storage_state,
            StorageStateV1::ColdEligible | StorageStateV1::Archiving
        ) && !storage_consent_active
        {
            return Err(
                "schedule schema: cold-eligible or archiving storage requires active consent"
                    .into(),
            );
        }
        if self.storage_state == StorageStateV1::Synchronized && !storage_consent_present {
            return Err(
                "schedule schema: synchronized storage requires a durable consent reference".into(),
            );
        }
        sha256("ledger headroom", &self.ledger_headroom_sha256)?;
        unique_ids(
            "status case id",
            self.cases.iter().map(|case| case.case_id.as_str()),
        )?;
        for case in &self.cases {
            optional_text("status last outcome", &case.last_outcome)?;
            optional_record_ref("status hold", &case.hold)?;
            optional_record_ref("status quarantine", &case.quarantine)?;
            let held = matches!(case.hold, OptionalRecordRefV1::Record { .. });
            let quarantined = matches!(case.quarantine, OptionalRecordRefV1::Record { .. });
            if held && quarantined {
                return Err(
                    "schedule schema: a status case cannot be simultaneously held and quarantined"
                        .into(),
                );
            }
            if (case.lifecycle == ScheduleCaseLifecycleV1::OperatorQuarantined) != quarantined {
                return Err(
                    "schedule schema: operator-quarantined lifecycle requires exactly one quarantine reference"
                        .into(),
                );
            }
            if matches!(
                case.lifecycle,
                ScheduleCaseLifecycleV1::CharacterizationRequired
                    | ScheduleCaseLifecycleV1::Deferred
                    | ScheduleCaseLifecycleV1::Retired
            ) && held
            {
                return Err(
                    "schedule schema: inactive case lifecycles cannot carry a live safety hold"
                        .into(),
                );
            }
        }
        let has_active_scheduled_case = self.cases.iter().any(|case| {
            matches!(
                case.lifecycle,
                ScheduleCaseLifecycleV1::ScheduledActive
                    | ScheduleCaseLifecycleV1::RequiredGateActive
            )
        });
        if has_active_scheduled_case
            && (!provider_grant_active
                || !matches!(&self.next_window, OptionalWindowV1::Window { .. }))
        {
            return Err(
                "schedule schema: active scheduled cases require an active grant and next window"
                    .into(),
            );
        }
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
            let expected = match rule.task_class {
                DogfoodTaskClassV1::BoundedSummaryDocsLightBrainstorm => (
                    DogfoodPrimaryModelClassV1::InexpensiveEligible,
                    DogfoodEffortPolicyV1::LowOrMedium,
                    DogfoodSecondOpinionV1::SolWhenScopeExpands,
                ),
                DogfoodTaskClassV1::SmallSpecifiedImplementation => (
                    DogfoodPrimaryModelClassV1::LunaOrSonnet,
                    DogfoodEffortPolicyV1::MediumOrHigh,
                    DogfoodSecondOpinionV1::SolForCrossCuttingDifficult,
                ),
                DogfoodTaskClassV1::NormalImplementation => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::High,
                    DogfoodSecondOpinionV1::OpusIndependentArchitecture,
                ),
                DogfoodTaskClassV1::SpecDesignArchitectureAuthoring => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::HighOrXhigh,
                    DogfoodSecondOpinionV1::OpusAssumptionsAlternativesGapsCrossCutting,
                ),
                DogfoodTaskClassV1::CleanroomSpecTechnicalDesign => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::HighOrXhigh,
                    DogfoodSecondOpinionV1::OpusIndependentNoNestedHelpers,
                ),
                DogfoodTaskClassV1::AdversarialDesignImplementationReview => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::Xhigh,
                    DogfoodSecondOpinionV1::OpusXhighThenFableHardComplex,
                ),
                DogfoodTaskClassV1::ReleaseCompatibilityReview => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::Xhigh,
                    DogfoodSecondOpinionV1::OpusOrFableAfterSolGreen,
                ),
                DogfoodTaskClassV1::FullBranchReview => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::Xhigh,
                    DogfoodSecondOpinionV1::OpusXhighThenFableRiskJustified,
                ),
                DogfoodTaskClassV1::RequirementsBrainstormAnalysisGrooming => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::HighOrXhigh,
                    DogfoodSecondOpinionV1::OpusAlternativesThenFableHardComplex,
                ),
                DogfoodTaskClassV1::ConcurrencyTransactionCriticalProof => (
                    DogfoodPrimaryModelClassV1::Sol,
                    DogfoodEffortPolicyV1::Max,
                    DogfoodSecondOpinionV1::FableAdversarialWhenUseful,
                ),
            };
            if (rule.primary_model_class, rule.effort, rule.second_opinion) != expected {
                return Err(format!(
                    "schedule schema: routing rule {:?} contradicts the approved task matrix",
                    rule.task_class
                )
                .into());
            }
        }
        Ok(())
    }
}

fn parse_and_validate<T: DeserializeOwned + ValidateRecord>(
    bytes: &[u8],
    label: &str,
) -> Result<(), BoxError> {
    let raw = std::str::from_utf8(bytes)
        .map_err(|_| format!("schedule schema: invalid {label}: JSON must be UTF-8"))?;
    if compatibility::looks_like_secret(raw) {
        return Err(format!("schedule schema: {label} contains secret-shaped material").into());
    }
    fn scan(value: &serde_json::Value, path: &str) -> Result<(), BoxError> {
        match value {
            serde_json::Value::String(value) => bounded_text(path, value),
            serde_json::Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    scan(value, &format!("{path}[{index}]"))?;
                }
                Ok(())
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    scan(value, &format!("{path}.{key}"))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    let untyped: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("schedule schema: invalid {label}: {error}"))?;
    scan(&untyped, label)?;
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

    fn text(value: &str) -> OptionalTextV1 {
        OptionalTextV1::Text {
            value: value.into(),
        }
    }

    fn sha1(ch: char) -> GitObjectIdV1 {
        GitObjectIdV1 {
            algorithm: GitObjectAlgorithmV1::Sha1,
            hex: ch.to_string().repeat(40),
        }
    }

    fn sha256_git(ch: char) -> GitObjectIdV1 {
        GitObjectIdV1 {
            algorithm: GitObjectAlgorithmV1::Sha256,
            hex: ch.to_string().repeat(64),
        }
    }

    fn test_merge() -> TestMergeIdentityV1 {
        let base = sha1('a');
        let head = sha1('b');
        TestMergeIdentityV1 {
            repository: "shoedog/a2acp".into(),
            pull_request: 37,
            base_oid: base.clone(),
            head_oid: head.clone(),
            merge_oid: sha1('c'),
            merge_ref: "refs/pull/37/merge".into(),
            tree_oid: sha1('d'),
            ordered_parents: vec![base, head],
        }
    }

    fn routing_policy() -> DogfoodRoutingPolicyV1 {
        use DogfoodEffortPolicyV1 as Effort;
        use DogfoodPrimaryModelClassV1 as Model;
        use DogfoodSecondOpinionV1 as Lens;
        use DogfoodTaskClassV1 as Task;
        let rules = [
            (
                Task::BoundedSummaryDocsLightBrainstorm,
                Model::InexpensiveEligible,
                Effort::LowOrMedium,
                Lens::SolWhenScopeExpands,
            ),
            (
                Task::SmallSpecifiedImplementation,
                Model::LunaOrSonnet,
                Effort::MediumOrHigh,
                Lens::SolForCrossCuttingDifficult,
            ),
            (
                Task::NormalImplementation,
                Model::Sol,
                Effort::High,
                Lens::OpusIndependentArchitecture,
            ),
            (
                Task::SpecDesignArchitectureAuthoring,
                Model::Sol,
                Effort::HighOrXhigh,
                Lens::OpusAssumptionsAlternativesGapsCrossCutting,
            ),
            (
                Task::CleanroomSpecTechnicalDesign,
                Model::Sol,
                Effort::HighOrXhigh,
                Lens::OpusIndependentNoNestedHelpers,
            ),
            (
                Task::AdversarialDesignImplementationReview,
                Model::Sol,
                Effort::Xhigh,
                Lens::OpusXhighThenFableHardComplex,
            ),
            (
                Task::ReleaseCompatibilityReview,
                Model::Sol,
                Effort::Xhigh,
                Lens::OpusOrFableAfterSolGreen,
            ),
            (
                Task::FullBranchReview,
                Model::Sol,
                Effort::Xhigh,
                Lens::OpusXhighThenFableRiskJustified,
            ),
            (
                Task::RequirementsBrainstormAnalysisGrooming,
                Model::Sol,
                Effort::HighOrXhigh,
                Lens::OpusAlternativesThenFableHardComplex,
            ),
            (
                Task::ConcurrencyTransactionCriticalProof,
                Model::Sol,
                Effort::Max,
                Lens::FableAdversarialWhenUseful,
            ),
        ]
        .into_iter()
        .map(
            |(task_class, primary_model_class, effort, second_opinion)| DogfoodRoutingRuleV1 {
                task_class,
                primary_model_class,
                effort,
                second_opinion,
            },
        )
        .collect();
        DogfoodRoutingPolicyV1 {
            schema_version: 1,
            advisory_only: true,
            audit_required: true,
            rules,
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
            max_tokens: attempts * 1_000,
            max_cost_microusd: attempts * 1_000,
            max_time_secs: attempts * 1_000,
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
                effort: text("low"),
                mode: OptionalTextV1::Absent,
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
                utc_day: aggregate_caps(3),
                rolling_24h: aggregate_caps(3),
                protected_scheduled: aggregate_caps(1),
                protected_test_merge: aggregate_caps(1),
                manual_unallocated: aggregate_caps(1),
            },
            confirmation_allowance: 1,
            launchd: vec![LaunchdBindingV1 {
                label: "com.a2a-bridge.compatibility.daily".into(),
                plist_sha256: digest('0'),
                trigger: TriggerKindV1::Daily,
            }],
            profiles: vec![profile],
            not_before_ms: 1,
            expires_at_ms: 100,
            revocation_generation: 1,
        }
    }

    fn publication_outbox() -> PublicationOutboxV1 {
        let mut outbox = PublicationOutboxV1 {
            schema_version: 1,
            outbox_id: "outbox:placeholder".into(),
            immutable_identity: fingerprint_value('0'),
            previous_record: OptionalSha256V1::Sha256 { value: digest('9') },
            state: PublicationOutboxStateV1::RemotePending,
            repository: "shoedog/a2acp".into(),
            pull_request: 37,
            test_merge_oid: sha1('a'),
            context: "a2a-bridge/r3d".into(),
            app_id: "app-1".into(),
            external_id: "external-1".into(),
            check_run: OptionalCheckRunIdV1::CheckRun { id: 1 },
            check_run_binding: OptionalSha256V1::Absent,
            terminal_consumption: OptionalStableIdV1::Absent,
            desired_conclusion: OptionalCheckConclusionV1::Absent,
            evidence_set: OptionalSha256V1::Absent,
            final_guard: OptionalSha256V1::Absent,
            remote_observation: OptionalSha256V1::Absent,
            remote_observation_attempts: 0,
        };
        let identity = publication_outbox_identity_sha256(&outbox).unwrap();
        outbox.outbox_id = format!("outbox:{identity}");
        outbox.immutable_identity.sha256 = identity;
        outbox.check_run_binding = OptionalSha256V1::Sha256 {
            value: publication_check_run_binding_sha256(&outbox.immutable_identity, 1).unwrap(),
        };
        outbox
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
                head_oid: sha1('a'),
                tree_oid: sha1('b'),
                range_start_exclusive: OptionalGitObjectIdV1::Absent,
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
        let sha256 = canonical_input_sha256("case-execution input", &input).unwrap();
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
        let sha256 = canonical_input_sha256("admission-attempt input", &input).unwrap();
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
    fn git_object_ids_require_nonnull_repository_wide_sha1_or_sha256() {
        let target = ExactExecutionTargetV1::RepositorySnapshot {
            repository: "shoedog/a2acp".into(),
            head_oid: GitObjectIdV1 {
                algorithm: GitObjectAlgorithmV1::Sha1,
                hex: "e7e5fa14da127511c080e802625afbcc455d94e1".into(),
            },
            tree_oid: sha1('a'),
            range_start_exclusive: OptionalGitObjectIdV1::ObjectId { value: sha1('b') },
        };
        validate_execution_target(&target).unwrap();

        let sha256_target = ExactExecutionTargetV1::RepositorySnapshot {
            repository: "sha256/repository".into(),
            head_oid: sha256_git('a'),
            tree_oid: sha256_git('b'),
            range_start_exclusive: OptionalGitObjectIdV1::ObjectId {
                value: sha256_git('c'),
            },
        };
        validate_execution_target(&sha256_target).unwrap();

        let mixed_target = ExactExecutionTargetV1::RepositorySnapshot {
            repository: "mixed/repository".into(),
            head_oid: sha1('a'),
            tree_oid: sha256_git('b'),
            range_start_exclusive: OptionalGitObjectIdV1::Absent,
        };
        assert!(validate_execution_target(&mixed_target)
            .unwrap_err()
            .to_string()
            .contains("mixes Git object algorithms"));

        let null = GitObjectIdV1 {
            algorithm: GitObjectAlgorithmV1::Sha1,
            hex: "0".repeat(40),
        };
        assert!(git_oid("null oid", &null).is_err());

        let mut invalid = sha1('c');
        invalid.algorithm = GitObjectAlgorithmV1::Sha256;
        assert!(git_oid("mismatched oid", &invalid).is_err());

        let merge = test_merge();
        validate_test_merge(&merge).unwrap();
        let mut wrong_parents = merge;
        wrong_parents.ordered_parents.reverse();
        assert!(validate_test_merge(&wrong_parents).is_err());
    }

    #[test]
    fn effective_identity_requires_explicit_tagged_absences() {
        let valid = serde_json::json!({
            "model": "gpt-5.6-luna",
            "effort": {"kind": "text", "value": "low"},
            "mode": {"kind": "absent"}
        });
        let identity: EffectiveIdentityV1 = serde_json::from_value(valid).unwrap();
        effective_identity("identity", &identity).unwrap();
        assert!(
            serde_json::from_value::<EffectiveIdentityV1>(serde_json::json!({
                "model": "gpt-5.6-luna",
                "effort": null,
                "mode": null
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<EffectiveIdentityV1>(serde_json::json!({
                "model": "gpt-5.6-luna"
            }))
            .is_err()
        );
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
            utc_day: aggregate_caps(3),
            rolling_24h: aggregate_caps(3),
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
        budgets.utc_day.max_attempts = 2;
        budgets.rolling_24h.max_attempts = 2;
        assert!(
            validate_grant_budgets(&budgets, &case_ids, &provider_families, &triggers)
                .unwrap_err()
                .to_string()
                .contains("combined")
        );

        for field in ["tokens", "cost", "time"] {
            let mut budgets = provider_grant().budgets;
            match field {
                "tokens" => {
                    budgets.utc_day.max_tokens -= 1;
                    budgets.rolling_24h.max_tokens -= 1;
                }
                "cost" => {
                    budgets.utc_day.max_cost_microusd -= 1;
                    budgets.rolling_24h.max_cost_microusd -= 1;
                }
                "time" => {
                    budgets.utc_day.max_time_secs -= 1;
                    budgets.rolling_24h.max_time_secs -= 1;
                }
                _ => unreachable!(),
            }
            assert!(
                validate_grant_budgets(&budgets, &case_ids, &provider_families, &triggers)
                    .unwrap_err()
                    .to_string()
                    .contains("combined"),
                "{field} allocation unexpectedly fit"
            );
        }
    }

    #[test]
    fn provider_grant_rejects_duplicate_cases_and_source_row_substitution() {
        let mut grant = provider_grant();
        grant.validate().unwrap();

        let mut missing_launchd = provider_grant();
        missing_launchd.launchd.clear();
        assert!(missing_launchd
            .validate()
            .unwrap_err()
            .to_string()
            .contains("launchd"));

        let mut wrong_row = provider_grant();
        wrong_row.profiles[0].source.row_id = "case-2".into();
        assert!(wrong_row
            .validate()
            .unwrap_err()
            .to_string()
            .contains("source row"));

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
            effort: text("low"),
            mode: OptionalTextV1::Absent,
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
    fn characterization_authorization_and_hold_quarantine_lifecycles_are_closed() {
        let source = ProfileSourceRefV1 {
            kind: ProfileSourceKindV1::ScheduledAdvisory,
            schema_version: 1,
            source_sha256: digest('a'),
            row_id: "case-1".into(),
            row_sha256: digest('b'),
        };
        let entry = OneShotCharacterizationEntryV1 {
            entry_id: "entry-1".into(),
            generation: 1,
            entry_sha256: digest('c'),
            consumption_nonce: "nonce-1".into(),
            source,
            characterization_profile: fingerprint_value('d'),
            characterization_execution: fingerprint_value('e'),
            proposed_effective_identity: EffectiveIdentityV1 {
                model: "gpt-5.6-luna".into(),
                effort: text("low"),
                mode: OptionalTextV1::Absent,
            },
            provider_family: "openai-codex".into(),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            caps: effect_caps(),
            command: "compatibility characterize".into(),
            not_before_ms: 1,
            expires_at_ms: 3,
            revocation_generation: 1,
        };
        let mut authorization = CharacterizationAuthorizationV1 {
            schema_version: 1,
            authorization_id: "authorization-1".into(),
            authorization_sha256: digest('f'),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            host_identity_sha256: digest('1'),
            profile_policy_bundle_sha256: digest('2'),
            scheduler_binary_sha256: digest('3'),
            price_snapshot_sha256: digest('4'),
            legacy_inventory_sha256: digest('5'),
            issued_at_ms: 1,
            entries: vec![entry],
        };
        authorization.validate().unwrap();
        authorization.entries[0].command = "compatibility run".into();
        assert!(authorization.validate().is_err());

        let mut hold = SafetyHoldV1 {
            schema_version: 1,
            hold_id: "hold-1".into(),
            characterization_profile: fingerprint_value('6'),
            case_execution: fingerprint_value('7'),
            reason: HoldReasonV1::ProcessExitUnproved,
            created_at_ms: 2,
            lifecycle: HoldLifecycleV1::Active,
        };
        hold.validate().unwrap();
        let opening_sha256 = safety_hold_opening_sha256(&hold).unwrap();
        let clearance_action_id = "clearance-1".to_owned();
        let cleared_at_ms = 3;
        let operator = "Wesley Jinks".to_owned();
        let reason = "process exit proved".to_owned();
        let clearance_action_sha256 = canonical_input_sha256(
            "safety-hold clearance action",
            &SafetyHoldClearanceIdentityV1 {
                schema_version: 1,
                opening_sha256: &opening_sha256,
                clearance_action_id: &clearance_action_id,
                cleared_at_ms,
                operator: &operator,
                reason: &reason,
            },
        )
        .unwrap();
        hold.lifecycle = HoldLifecycleV1::Cleared {
            opening_sha256,
            clearance_action_id,
            clearance_action_sha256,
            cleared_at_ms,
            operator,
            reason,
        };
        hold.validate().unwrap();
        let valid_clearance = hold.lifecycle.clone();
        if let HoldLifecycleV1::Cleared { opening_sha256, .. } = &mut hold.lifecycle {
            *opening_sha256 = digest('a');
        }
        assert!(hold.validate().is_err());
        hold.lifecycle = valid_clearance.clone();
        if let HoldLifecycleV1::Cleared {
            clearance_action_sha256,
            ..
        } = &mut hold.lifecycle
        {
            *clearance_action_sha256 = digest('b');
        }
        assert!(hold.validate().is_err());
        hold.lifecycle = valid_clearance;
        if let HoldLifecycleV1::Cleared { cleared_at_ms, .. } = &mut hold.lifecycle {
            *cleared_at_ms = 1;
        }
        assert!(hold.validate().is_err());

        let open = QuarantineV1::Open {
            schema_version: 1,
            quarantine_id: "quarantine-1".into(),
            profile: fingerprint_value('8'),
            operator: "Wesley Jinks".into(),
            reason: "owner review".into(),
            created_at_ms: 1,
            expires_at_ms: 10,
        };
        open.validate().unwrap();
        let mut closed = QuarantineV1::Closed {
            schema_version: 1,
            quarantine_id: "quarantine-1".into(),
            profile: fingerprint_value('8'),
            opening_sha256: digest('9'),
            operator: "Wesley Jinks".into(),
            reason: "owner cleared".into(),
            created_at_ms: 1,
            closed_at_ms: 2,
        };
        closed.validate().unwrap();
        if let QuarantineV1::Closed { closed_at_ms, .. } = &mut closed {
            *closed_at_ms = 0;
        }
        assert!(closed.validate().is_err());
    }

    #[test]
    fn impact_reducer_enforces_docs_tests_production_and_new_provider_states() {
        let mut record = ImpactRecordV1 {
            schema_version: 1,
            classifier_sha256: digest('a'),
            target: test_merge(),
            classes: vec![ImpactClassV1::DocumentationOnly],
            due_case_ids: Vec::new(),
            characterization_required_case_ids: Vec::new(),
            no_impact_proved: true,
        };
        record.validate().unwrap();
        record
            .due_case_ids
            .push("codex-host-bridge-gpt56-sol".into());
        record.no_impact_proved = false;
        assert!(record.validate().is_err());

        record.classes = vec![ImpactClassV1::TestsOnly, ImpactClassV1::CompatibilityCore];
        record.due_case_ids = EXPECTED_SUPPORT_PROFILES.map(str::to_owned).to_vec();
        record.no_impact_proved = false;
        record.validate().unwrap();

        record.classes = vec![ImpactClassV1::NewProvider];
        record.due_case_ids.clear();
        record.characterization_required_case_ids = vec!["openrouter-new".into()];
        record.validate().unwrap();
        record.characterization_required_case_ids.clear();
        assert!(record.validate().is_err());

        record.classes = vec![ImpactClassV1::CompatibilityCore];
        assert!(record.validate().is_err());

        record.classes = vec![ImpactClassV1::ContainerRuntime];
        record.due_case_ids = vec![
            "claude-reader-055-fable".into(),
            "codex-reader-bridge-gpt56-sol".into(),
            "codex-host-bridge-gpt56-sol".into(),
        ];
        assert!(record.validate().is_err());
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
            source: ManualAdmissionSourceV1::DirectLocalCli,
            command: ManualCommandV1::CompatibilityRun,
            caps: effect_caps(),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            retry_cap: 0,
            fallback_cap: 0,
            acknowledged_billable: true,
            issued_at_ms: 1,
            expires_at_ms: 2,
        };
        manual.validate().unwrap();
        let mut arbitrary = serde_json::to_value(&manual).unwrap();
        arbitrary["command"] = serde_json::json!("implement");
        assert!(serde_json::from_value::<ManualAdmissionV1>(arbitrary).is_err());
        let mut remote = serde_json::to_value(&manual).unwrap();
        remote["source"] = serde_json::json!("serve");
        assert!(serde_json::from_value::<ManualAdmissionV1>(remote).is_err());
        manual.evidence_purpose = EvidencePurposeV1::Characterization;
        assert!(manual
            .validate()
            .unwrap_err()
            .to_string()
            .contains("cannot authorize characterization"));

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
            provenance: ConsumptionEvidenceProvenanceV1::Ordinary,
            characterization_profile: fingerprint_value('2'),
            case_execution: fingerprint_value('3'),
            admission_attempt: fingerprint_value('4'),
            authority: standing_authority(),
            consumed_at_ms: 1,
        };
        consumption.validate().unwrap();
        consumption.requested_purpose = EvidencePurposeV1::ManualDiagnostic;
        assert!(consumption.validate().is_err());

        consumption.requested_purpose = EvidencePurposeV1::ProviderPathAdvisory;
        consumption.satisfied_purpose = EvidencePurposeV1::Characterization;
        consumption.consumed_at_ms = 30;
        consumption.provenance = ConsumptionEvidenceProvenanceV1::ReviewedCharacterization {
            characterization_id: "characterization-1".into(),
            characterization_record_sha256: digest('5'),
            freshness_bucket: "policy-window-1".into(),
            freshness_observation_sha256: digest('6'),
            terminal_at_ms: 10,
            reviewed_at_ms: 20,
            reviewer: "Wesley Jinks".into(),
        };
        consumption.validate().unwrap();

        let mut unreviewed = consumption.clone();
        unreviewed.provenance = ConsumptionEvidenceProvenanceV1::Ordinary;
        assert!(unreviewed.validate().is_err());

        let ConsumptionEvidenceProvenanceV1::ReviewedCharacterization {
            freshness_observation_sha256,
            ..
        } = &mut consumption.provenance
        else {
            unreachable!()
        };
        *freshness_observation_sha256 = "not-a-digest".into();
        assert!(consumption.validate().is_err());
    }

    #[test]
    fn ledger_reservation_and_equivalent_work_records_bind_all_identity_layers() {
        let authority = standing_authority();
        let reservation = LedgerRecordV1::Reservation(LedgerReservationV1 {
            schema_version: 1,
            reservation_id: "reservation-1".into(),
            attempt_idempotency_key: digest('a'),
            accounting_class: AccountingClassV1::Scheduled,
            characterization_profile: fingerprint_value('b'),
            case_execution: fingerprint_value('c'),
            admission_attempt: fingerprint_value('d'),
            authority: authority.clone(),
            equivalent_work_key: digest('e'),
            evidence_purpose: EvidencePurposeV1::ProviderPathAdvisory,
            freshness_bucket: "window-1".into(),
            repeat_nonce: OptionalStableIdV1::Absent,
            caps: effect_caps(),
            utc_day_id: "2026-07-18".into(),
            rolling_window_id: "rolling-1".into(),
            reserved_at_ms: 1,
        });
        reservation.validate().unwrap();

        let mut equivalent = EquivalentWorkReservationV1 {
            schema_version: 1,
            reservation_id: "equivalent-1".into(),
            equivalent_work_key: digest('f'),
            characterization_profile: fingerprint_value('1'),
            case_execution: fingerprint_value('2'),
            admission_attempt: fingerprint_value('3'),
            evidence_purpose: EvidencePurposeV1::ProviderPathAdvisory,
            freshness_bucket: "window-1".into(),
            authority,
            reserved_at_ms: 1,
        };
        equivalent.validate().unwrap();
        equivalent.equivalent_work_key = "not-a-digest".into();
        assert!(equivalent.validate().is_err());
    }

    #[test]
    fn claimed_support_source_and_schedule_sidecar_validate_cross_layer_bindings() {
        let claimed_source = ProfileSourceRefV1 {
            kind: ProfileSourceKindV1::ClaimedSupportGate,
            schema_version: 1,
            source_sha256: digest('a'),
            row_id: "case-1".into(),
            row_sha256: digest('b'),
        };
        let profile = fingerprint_value('c');
        let identity = EffectiveIdentityV1 {
            model: "gpt-5.6-sol".into(),
            effort: text("xhigh"),
            mode: text("read-only"),
        };
        let caps = effect_caps();
        let execution = execution_record(
            profile.clone(),
            claimed_source.source_sha256.clone(),
            claimed_source.row_sha256.clone(),
            identity.clone(),
            caps.clone(),
        );
        let authority = one_shot_authority();
        let admission = admission_record(
            profile.clone(),
            execution.fingerprint.clone(),
            authority.clone(),
            TriggerSourceV1::ManualCharacterizationCli,
            TriggerKindV1::ManualCharacterization,
        );
        let mut claimed = ClaimedSupportCharacterizationSourceV1 {
            schema_version: 1,
            source_sha256: digest('d'),
            source: claimed_source,
            production_manifest_sha256: digest('a'),
            profile_policy_bundle_sha256: digest('e'),
            characterization_profile: profile,
            characterization_execution: execution,
            admission_attempt: admission,
            authority,
            trigger: TriggerKindV1::ManualCharacterization,
            pinned_config_sha256: digest('f'),
            requested_identity: identity.clone(),
            expected_effective_identity: identity,
            caps,
        };
        claimed.validate().unwrap();
        claimed.pinned_config_sha256 = digest('9');
        assert!(claimed.validate().is_err());

        let mut sidecar = ScheduleEvidenceRecordV1 {
            schema_version: 1,
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
            characterization_profile: fingerprint_value('4'),
            case_execution: fingerprint_value('5'),
            admission_attempt: fingerprint_value('6'),
            authority: standing_authority(),
            aggregate: OptionalSha256V1::Absent,
            evidence_index_id: "index-1".into(),
            check: CheckBindingV1::Absent,
            storage_consent: OptionalRecordRefV1::Absent,
            quarantine: OptionalRecordRefV1::Absent,
            characterization: OptionalRecordRefV1::Absent,
            window_id: "window-1".into(),
            attempt_idempotency_key: digest('7'),
            equivalent_work_key: digest('8'),
            consumption: OptionalRecordRefV1::Absent,
            repeat_nonce: OptionalStableIdV1::Absent,
            ledger_reservation_id: "reservation-1".into(),
            budget_reservation_sha256: digest('9'),
            ledger_reconciliation: OptionalSha256V1::Absent,
            deadline_derivation_sha256: digest('a'),
            preflight_results_sha256: digest('b'),
            admission_lock_holder_sha256: digest('c'),
            supervisor_record_sha256: digest('d'),
            freshness_observation_sha256: digest('e'),
            requested_identity: EffectiveIdentityV1 {
                model: "gpt-5.6-luna".into(),
                effort: text("low"),
                mode: OptionalTextV1::Absent,
            },
            expected_effective_identity: EffectiveIdentityV1 {
                model: "gpt-5.6-luna".into(),
                effort: text("low"),
                mode: OptionalTextV1::Absent,
            },
            observed_effective_identity: OptionalEffectiveIdentityV1::Absent,
            publication_outbox: OptionalRecordRefV1::Absent,
            status_publication: OptionalSha256V1::Absent,
            affected_case_ids: vec!["case-1".into()],
            created_at_ms: 1,
        };
        sidecar.validate().unwrap();
        sidecar.trigger = TriggerKindV1::TestMerge;
        assert!(sidecar.validate().is_err());
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
            action: FailureActionV1::UnknownRetained,
            ..first_untyped
        };
        second_untyped.validate().unwrap();
        let suppressed = FailureDispositionV1 {
            action: FailureActionV1::Suppressed,
            ..second_untyped
        };
        assert!(suppressed.validate().is_err());
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
            effort: text("low"),
            mode: OptionalTextV1::Absent,
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
    fn publication_outbox_binds_immutable_remote_identity_and_transition_chain() {
        let outbox = publication_outbox();
        outbox.validate().unwrap();

        let mut changed_repository = outbox.clone();
        changed_repository.repository = "shoedog/another-repository".into();
        assert!(changed_repository
            .validate()
            .unwrap_err()
            .to_string()
            .contains("immutable remote identity"));

        let mut changed_check_run = outbox.clone();
        changed_check_run.check_run = OptionalCheckRunIdV1::CheckRun { id: 2 };
        assert!(changed_check_run
            .validate()
            .unwrap_err()
            .to_string()
            .contains("check-run id"));

        let mut unchained = outbox.clone();
        unchained.previous_record = OptionalSha256V1::Absent;
        assert!(unchained.validate().is_err());

        let mut create_intent = outbox;
        create_intent.state = PublicationOutboxStateV1::CreateIntent;
        create_intent.previous_record = OptionalSha256V1::Absent;
        create_intent.check_run = OptionalCheckRunIdV1::Absent;
        create_intent.check_run_binding = OptionalSha256V1::Absent;
        create_intent.validate().unwrap();
    }

    #[test]
    fn evidence_paths_are_root_bound_normalized_and_portable() {
        let entry = EvidenceIndexEntryV1 {
            evidence_id: "evidence-1".into(),
            evidence_class: EvidenceClassV1::RoutineGreen,
            full_evidence_sha256: digest('a'),
            compact_record_sha256: digest('b'),
            hot_path: RelativeEvidencePathV1 {
                components: vec!["2026".into(), "evidence-1.json".into()],
            },
            cold_path: OptionalRelativeEvidencePathV1::Absent,
            full_retain_until_ms: 10,
            compact_retain_until_ms: 20,
            pinned: false,
            lease_count: 0,
        };
        let mut index = EvidenceIndexV1 {
            schema_version: 1,
            index_id: "index-1".into(),
            generation: 1,
            hot_root_sha256: digest('c'),
            cold_storage: ColdStorageBindingV1::Absent,
            entries: vec![entry],
        };
        index.validate().unwrap();

        index.entries[0].hot_path.components[0] = "..".into();
        assert!(index.validate().is_err());
        index.entries[0].hot_path.components[0] = "CON".into();
        assert!(index.validate().is_err());
        index.entries[0].hot_path.components[0] = "2026".into();
        index.entries[0].cold_path = OptionalRelativeEvidencePathV1::RelativePath {
            value: RelativeEvidencePathV1 {
                components: vec!["archive".into(), "evidence-1.json".into()],
            },
        };
        assert!(index.validate().is_err());
        index.cold_storage = ColdStorageBindingV1::OwnerIcloud {
            consent_id: "consent-1".into(),
            consent_sha256: digest('d'),
            root_sha256: digest('e'),
            file_provider_domain_id: "icloud-drive".into(),
        };
        index.validate().unwrap();

        let original = index.entries[0].clone();
        let mut duplicate_hot = original.clone();
        duplicate_hot.evidence_id = "evidence-2".into();
        duplicate_hot.full_evidence_sha256 = digest('f');
        duplicate_hot.compact_record_sha256 = digest('1');
        index.entries.push(duplicate_hot);
        assert!(index
            .validate()
            .unwrap_err()
            .to_string()
            .contains("hot evidence paths"));

        index.entries[1].hot_path.components = vec!["2026".into(), "EVIDENCE-1.JSON".into()];
        assert!(index.validate().is_err());

        index.entries[1].hot_path.components = vec!["2026".into(), "evidence-2.json".into()];
        assert!(index
            .validate()
            .unwrap_err()
            .to_string()
            .contains("cold evidence paths"));

        index.entries.truncate(1);
        index.entries[0].hot_path.components[0] = "café".into();
        assert!(index.validate().is_err());
    }

    #[test]
    fn status_enforces_windows_authority_expiry_and_case_control_coherence() {
        let authority = OptionalAuthorityStatusV1::Authority {
            id: "grant-1".into(),
            sha256: digest('a'),
            state: AuthorityStateV1::Active,
            expires_at_ms: 100,
            revocation_generation: 1,
        };
        let mut status = ScheduleStatusV1 {
            schema_version: 1,
            generated_at_ms: 10,
            policy_sha256: digest('b'),
            last_window: OptionalWindowV1::Window {
                id: "window-1".into(),
                scheduled_at_ms: 5,
            },
            next_window: OptionalWindowV1::Window {
                id: "window-2".into(),
                scheduled_at_ms: 20,
            },
            provider_grant: authority,
            storage_consent: OptionalAuthorityStatusV1::Absent,
            ledger_headroom_sha256: digest('c'),
            storage_state: StorageStateV1::HotOnly,
            missed_ticks: 0,
            fresh_one_shot_compatibility: OneShotCompatibilityStateV1::Unknown,
            shared_operator_health: SharedOperatorHealthV1::NotEvaluated,
            cases: vec![ScheduleCaseStatusV1 {
                case_id: "case-1".into(),
                lifecycle: ScheduleCaseLifecycleV1::ScheduledActive,
                last_outcome: OptionalTextV1::Absent,
                hold: OptionalRecordRefV1::Absent,
                quarantine: OptionalRecordRefV1::Absent,
            }],
        };
        status.validate().unwrap();
        let valid_status = status.clone();

        status.cases[0].hold = OptionalRecordRefV1::Record {
            id: "hold-1".into(),
            sha256: digest('d'),
        };
        status.cases[0].quarantine = OptionalRecordRefV1::Record {
            id: "quarantine-1".into(),
            sha256: digest('e'),
        };
        assert!(status.validate().is_err());
        status.cases[0].hold = OptionalRecordRefV1::Absent;
        status.cases[0].lifecycle = ScheduleCaseLifecycleV1::OperatorQuarantined;
        status.validate().unwrap();
        status.cases[0].quarantine = OptionalRecordRefV1::Absent;
        assert!(status.validate().is_err());
        status.cases[0].lifecycle = ScheduleCaseLifecycleV1::ScheduledActive;

        let OptionalAuthorityStatusV1::Authority {
            state,
            expires_at_ms,
            ..
        } = &mut status.provider_grant
        else {
            unreachable!()
        };
        *state = AuthorityStateV1::Active;
        *expires_at_ms = 9;
        assert!(status.validate().is_err());

        let mut future_last = valid_status.clone();
        future_last.last_window = OptionalWindowV1::Window {
            id: "window-future-last".into(),
            scheduled_at_ms: 20,
        };
        future_last.next_window = OptionalWindowV1::Window {
            id: "window-future-next".into(),
            scheduled_at_ms: 30,
        };
        assert!(future_last.validate().is_err());

        let mut missing_grant = valid_status.clone();
        missing_grant.provider_grant = OptionalAuthorityStatusV1::Absent;
        assert!(missing_grant.validate().is_err());

        let mut missing_next = valid_status.clone();
        missing_next.next_window = OptionalWindowV1::Absent;
        assert!(missing_next.validate().is_err());

        let mut unconsented_cold = valid_status.clone();
        unconsented_cold.storage_state = StorageStateV1::ColdEligible;
        assert!(unconsented_cold.validate().is_err());
        unconsented_cold.storage_state = StorageStateV1::Synchronized;
        assert!(unconsented_cold.validate().is_err());

        let mut revoked = valid_status;
        revoked.cases[0].lifecycle = ScheduleCaseLifecycleV1::Deferred;
        let OptionalAuthorityStatusV1::Authority {
            state,
            expires_at_ms,
            ..
        } = &mut revoked.provider_grant
        else {
            unreachable!()
        };
        *state = AuthorityStateV1::Revoked;
        *expires_at_ms = 9;
        revoked.validate().unwrap();
    }

    #[test]
    fn routing_policy_is_the_exact_approved_matrix() {
        let mut policy = routing_policy();
        policy.validate().unwrap();
        let normal = policy
            .rules
            .iter_mut()
            .find(|rule| rule.task_class == DogfoodTaskClassV1::NormalImplementation)
            .unwrap();
        normal.primary_model_class = DogfoodPrimaryModelClassV1::InexpensiveEligible;
        assert!(policy.validate().is_err());

        let mut policy = routing_policy();
        let release = policy
            .rules
            .iter_mut()
            .find(|rule| rule.task_class == DogfoodTaskClassV1::ReleaseCompatibilityReview)
            .unwrap();
        release.effort = DogfoodEffortPolicyV1::LowOrMedium;
        assert!(policy.validate().is_err());
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

    #[test]
    fn record_parser_secret_scans_every_string_before_typed_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbox.json");
        let mut outbox = serde_json::to_value(publication_outbox()).unwrap();
        outbox["repository"] = serde_json::json!("sk-live-abcdefghijklmnopqrstuvwxyz123456");
        std::fs::write(&path, serde_json::to_vec(&outbox).unwrap()).unwrap();
        let error = validate_schedule_record("publication-outbox", &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("secret-shaped"), "{error}");

        let mut nested = serde_json::to_value(publication_outbox()).unwrap();
        nested["unexpected_nested"] = serde_json::json!({"password": "hunter2"});
        std::fs::write(&path, serde_json::to_vec(&nested).unwrap()).unwrap();
        let error = validate_schedule_record("publication-outbox", &path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("secret-shaped"), "{error}");
    }
}
