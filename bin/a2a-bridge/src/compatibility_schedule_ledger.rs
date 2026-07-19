//! Crash-conservative owner-local accounting for R3d2 compatibility effects.
//!
//! Reservations and reconciliations are separate create-new, mode-0600 records. The authoritative
//! view is rebuilt from those immutable records under the owner-wide admission lock; no mutable
//! projection can create headroom. An absent reconciliation is always a full conservative charge.

#![allow(dead_code)] // R3d2e wires these transaction capabilities after this checkpoint.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Write as _;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule::{EffectCapsV1, TriggerKindV1};
use crate::compatibility_schedule_admission::{
    DerivedAdmissionIdentitiesV1, DerivedLedgerAdmissionContextV1,
};
use crate::compatibility_schedule_schema::{
    AccountingClassV1, AdmissionAuthorityV1, AggregateBudgetCapsV1, GrantBudgetPolicyV1,
    LedgerDispositionV1, LedgerReconciliationV1, LedgerRecordV1, LedgerReservationV1,
    TriggerBudgetCapsV1, UsageChargeV1, ValidateRecord,
};
use crate::compatibility_schedule_state::AdmissionStateCapability;
use crate::{local_file, BoxError};

const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;
const MAX_LEDGER_FILES: usize = 100_000;
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum LedgerBudgetAuthorityV1 {
    CharacterizationOnce {
        entry_sha256: String,
        case_id: String,
        provider_family: String,
        caps: EffectCapsV1,
    },
    StandingGrant {
        grant_sha256: String,
        budgets: GrantBudgetPolicyV1,
    },
    ManualUnallocated {
        manual_admission_sha256: String,
        accounting_grant_sha256: String,
        budgets: GrantBudgetPolicyV1,
    },
}

#[derive(Clone, Debug)]
pub(super) struct LedgerReservationRequestV1<'a> {
    identities: &'a DerivedAdmissionIdentitiesV1,
    accounting_class: AccountingClassV1,
    case_id: &'a str,
    provider_family: &'a str,
    budget_authority: &'a LedgerBudgetAuthorityV1,
}

impl<'a> LedgerReservationRequestV1<'a> {
    pub(super) fn from_derived_context(
        context: &'a DerivedLedgerAdmissionContextV1,
        budget_authority: &'a LedgerBudgetAuthorityV1,
    ) -> Self {
        Self {
            identities: &context.identities,
            accounting_class: context.accounting_class,
            case_id: &context.case_id,
            provider_family: &context.provider_family,
            budget_authority,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConservativeChargeReasonV1 {
    SpawnStateAmbiguous,
    PromptAcceptancePossible,
    KillOrCrash,
    MissingUsage,
    InvalidUsage,
    UnknownPrice,
    UnknownCurrency,
    EvidenceUnreconciled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ReconciliationDecisionV1 {
    ProvedPreEffect {
        evidence_sha256: String,
        reconciled_at_ms: i64,
    },
    ValidTerminal {
        evidence_sha256: String,
        usage: UsageChargeV1,
        prompt_was_accepted: bool,
        reconciled_at_ms: i64,
    },
    Conservative {
        evidence_sha256: String,
        reason: ConservativeChargeReasonV1,
        prompt_may_have_been_accepted: bool,
        reconciled_at_ms: i64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct AggregateUsageV1 {
    pub(super) attempts: u64,
    pub(super) tokens: u64,
    pub(super) cost_microusd: u64,
    pub(super) time_secs: u64,
}

impl AggregateUsageV1 {
    fn from_caps(value: &EffectCapsV1) -> Self {
        Self {
            attempts: u64::from(value.attempts),
            tokens: value.max_tokens,
            cost_microusd: value.max_cost_microusd,
            time_secs: value.timeout_secs,
        }
    }

    fn from_charge(value: &UsageChargeV1) -> Self {
        Self {
            attempts: u64::from(value.attempts),
            tokens: value.tokens,
            cost_microusd: value.cost_microusd,
            time_secs: value.elapsed_millis.div_ceil(1_000),
        }
    }

    fn checked_add(self, other: Self) -> Result<Self, BoxError> {
        Ok(Self {
            attempts: self
                .attempts
                .checked_add(other.attempts)
                .ok_or("schedule ledger: attempt total overflow")?,
            tokens: self
                .tokens
                .checked_add(other.tokens)
                .ok_or("schedule ledger: token total overflow")?,
            cost_microusd: self
                .cost_microusd
                .checked_add(other.cost_microusd)
                .ok_or("schedule ledger: cost total overflow")?,
            time_secs: self
                .time_secs
                .checked_add(other.time_secs)
                .ok_or("schedule ledger: time total overflow")?,
        })
    }

    fn within(self, caps: &AggregateBudgetCapsV1) -> bool {
        self.attempts <= caps.max_attempts
            && self.tokens <= caps.max_tokens
            && self.cost_microusd <= caps.max_cost_microusd
            && self.time_secs <= caps.max_time_secs
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LedgerAdmissionRefusalV1 {
    ClockRollback,
    UtcDayExhausted,
    Rolling24hExhausted,
    PerCaseExhausted,
    PerProviderExhausted,
    PerTriggerExhausted,
    CharacterizationExhausted,
    ProtectedScheduledExhausted,
    ProtectedTestMergeExhausted,
    ManualUnallocatedExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LedgerAppendOutcomeV1 {
    Created,
    ExistingIdentical,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacyImportKindV1 {
    ValidatedAggregate,
    ConservativeAggregate,
    UnknownInitialRollingWindow,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyLedgerImportV1 {
    pub(super) schema_version: u16,
    pub(super) import_id: String,
    pub(super) inventory_sha256: String,
    pub(super) aggregate_sha256: String,
    pub(super) kind: LegacyImportKindV1,
    pub(super) case_id: String,
    pub(super) provider_family: String,
    pub(super) trigger: TriggerKindV1,
    pub(super) accounting_class: AccountingClassV1,
    pub(super) charged_usage: AggregateUsageV1,
    pub(super) observed_at_ms: i64,
    pub(super) rolling_expires_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LegacyLedgerImportInputV1 {
    pub(super) inventory_sha256: String,
    pub(super) aggregate_sha256: String,
    pub(super) kind: LegacyImportKindV1,
    pub(super) case_id: String,
    pub(super) provider_family: String,
    pub(super) trigger: TriggerKindV1,
    pub(super) accounting_class: AccountingClassV1,
    pub(super) charged_usage: AggregateUsageV1,
    pub(super) observed_at_ms: i64,
}

fn ledger_hash<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| format!("schedule ledger: cannot canonicalize {label}: {error}"))?;
    let mut bytes = format!("a2a-bridge:r3d2:ledger:{label}:v1\0").into_bytes();
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
        return Err(format!("schedule ledger: {label} is not a bounded stable id").into());
    }
    Ok(())
}

fn aggregate_caps_valid(value: &AggregateBudgetCapsV1) -> bool {
    value.max_attempts > 0
        && value.max_attempts <= 10_000
        && value.max_tokens > 0
        && value.max_tokens <= 1_000_000_000
        && value.max_cost_microusd <= 10_000_000_000
        && value.max_time_secs > 0
        && value.max_time_secs <= 31 * 24 * 60 * 60
}

fn add_caps(
    left: &AggregateBudgetCapsV1,
    right: &AggregateBudgetCapsV1,
) -> Result<AggregateBudgetCapsV1, BoxError> {
    Ok(AggregateBudgetCapsV1 {
        max_attempts: left
            .max_attempts
            .checked_add(right.max_attempts)
            .ok_or("schedule ledger: allocated attempt cap overflow")?,
        max_tokens: left
            .max_tokens
            .checked_add(right.max_tokens)
            .ok_or("schedule ledger: allocated token cap overflow")?,
        max_cost_microusd: left
            .max_cost_microusd
            .checked_add(right.max_cost_microusd)
            .ok_or("schedule ledger: allocated cost cap overflow")?,
        max_time_secs: left
            .max_time_secs
            .checked_add(right.max_time_secs)
            .ok_or("schedule ledger: allocated time cap overflow")?,
    })
}

fn caps_within(value: &AggregateBudgetCapsV1, maximum: &AggregateBudgetCapsV1) -> bool {
    value.max_attempts <= maximum.max_attempts
        && value.max_tokens <= maximum.max_tokens
        && value.max_cost_microusd <= maximum.max_cost_microusd
        && value.max_time_secs <= maximum.max_time_secs
}

fn validate_budget_policy(value: &GrantBudgetPolicyV1) -> Result<(), BoxError> {
    let all = value
        .per_case
        .iter()
        .map(|entry| &entry.caps)
        .chain(value.per_provider.iter().map(|entry| &entry.caps))
        .chain(value.per_trigger_pool.iter().map(|entry| &entry.caps))
        .chain([
            &value.utc_day,
            &value.rolling_24h,
            &value.protected_scheduled,
            &value.protected_test_merge,
            &value.manual_unallocated,
        ]);
    if all.into_iter().any(|caps| !aggregate_caps_valid(caps))
        || !caps_within(&value.rolling_24h, &value.utc_day)
        || value
            .per_case
            .iter()
            .chain(value.per_provider.iter())
            .any(|entry| !caps_within(&entry.caps, &value.utc_day))
        || value
            .per_trigger_pool
            .iter()
            .any(|entry| !caps_within(&entry.caps, &value.utc_day))
    {
        return Err("schedule ledger: budget policy has invalid aggregate caps".into());
    }
    let mut allocated = add_caps(&value.protected_scheduled, &value.protected_test_merge)?;
    allocated = add_caps(&allocated, &value.manual_unallocated)?;
    if !caps_within(&allocated, &value.utc_day) || !caps_within(&allocated, &value.rolling_24h) {
        return Err("schedule ledger: protected/manual allocations exceed shared caps".into());
    }
    let unique_named = |values: &[crate::compatibility_schedule_schema::NamedBudgetCapsV1]| {
        let set = values
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<BTreeSet<_>>();
        set.len() == values.len()
            && values
                .iter()
                .all(|entry| stable_id("budget dimension", &entry.id).is_ok())
    };
    let triggers = value
        .per_trigger_pool
        .iter()
        .map(|entry| entry.trigger)
        .collect::<BTreeSet<_>>();
    if !unique_named(&value.per_case)
        || !unique_named(&value.per_provider)
        || triggers.len() != value.per_trigger_pool.len()
    {
        return Err("schedule ledger: budget dimensions are duplicated or invalid".into());
    }
    Ok(())
}

fn policy_sha256(value: &LedgerBudgetAuthorityV1) -> Result<String, BoxError> {
    ledger_hash("accounting-policy", value)
}

fn aggregate_from_effect_caps(value: &EffectCapsV1) -> AggregateBudgetCapsV1 {
    AggregateBudgetCapsV1 {
        max_attempts: u64::from(value.attempts),
        max_tokens: value.max_tokens,
        max_cost_microusd: value.max_cost_microusd,
        max_time_secs: value.timeout_secs,
    }
}

fn find_named<'a>(
    values: &'a [crate::compatibility_schedule_schema::NamedBudgetCapsV1],
    id: &str,
) -> Option<&'a AggregateBudgetCapsV1> {
    values
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| &entry.caps)
}

fn find_trigger(
    values: &[TriggerBudgetCapsV1],
    trigger: TriggerKindV1,
) -> Option<&AggregateBudgetCapsV1> {
    values
        .iter()
        .find(|entry| entry.trigger == trigger)
        .map(|entry| &entry.caps)
}

#[derive(Clone)]
struct BudgetLimits {
    utc_day: AggregateBudgetCapsV1,
    rolling_24h: AggregateBudgetCapsV1,
    per_case: AggregateBudgetCapsV1,
    per_provider: AggregateBudgetCapsV1,
    per_trigger: Option<AggregateBudgetCapsV1>,
    class_pool: Option<(AggregateBudgetCapsV1, LedgerAdmissionRefusalV1)>,
}

impl LedgerBudgetAuthorityV1 {
    fn limits(&self, request: &LedgerReservationRequestV1<'_>) -> Result<BudgetLimits, BoxError> {
        let authority = &request.identities.admission_attempt.input.authority;
        let trigger = request.identities.admission_attempt.input.trigger.kind;
        let actual_caps = &request.identities.case_execution.input.actual_caps;
        actual_caps.validate("ledger reservation caps")?;
        match self {
            Self::CharacterizationOnce {
                entry_sha256,
                case_id,
                provider_family,
                caps,
            } => {
                let AdmissionAuthorityV1::CharacterizationOnce(bound) = authority else {
                    return Err(
                        "schedule ledger: characterization policy has wrong authority".into(),
                    );
                };
                if request.accounting_class != AccountingClassV1::Characterization
                    || trigger != TriggerKindV1::ManualCharacterization
                    || &bound.entry_sha256 != entry_sha256
                    || request.case_id != case_id
                    || request.provider_family != provider_family
                    || actual_caps != caps
                {
                    return Err("schedule ledger: characterization policy bindings diverged".into());
                }
                let caps = aggregate_from_effect_caps(caps);
                Ok(BudgetLimits {
                    utc_day: caps.clone(),
                    rolling_24h: caps.clone(),
                    per_case: caps.clone(),
                    per_provider: caps.clone(),
                    per_trigger: Some(caps.clone()),
                    class_pool: Some((caps, LedgerAdmissionRefusalV1::CharacterizationExhausted)),
                })
            }
            Self::StandingGrant {
                grant_sha256,
                budgets,
            } => {
                let AdmissionAuthorityV1::StandingGrant(bound) = authority else {
                    return Err("schedule ledger: standing policy has wrong authority".into());
                };
                if &bound.grant_sha256 != grant_sha256
                    || !matches!(
                        (request.accounting_class, trigger),
                        (AccountingClassV1::Scheduled, TriggerKindV1::Daily)
                            | (AccountingClassV1::Scheduled, TriggerKindV1::ScheduledMain)
                            | (AccountingClassV1::TestMerge, TriggerKindV1::TestMerge)
                    )
                {
                    return Err("schedule ledger: standing policy bindings diverged".into());
                }
                validate_budget_policy(budgets)?;
                let per_case = find_named(&budgets.per_case, request.case_id)
                    .ok_or("schedule ledger: standing policy has no case cap")?;
                let per_provider = find_named(&budgets.per_provider, request.provider_family)
                    .ok_or("schedule ledger: standing policy has no provider cap")?;
                let per_trigger = find_trigger(&budgets.per_trigger_pool, trigger)
                    .ok_or("schedule ledger: standing policy has no trigger cap")?;
                let class_pool = match request.accounting_class {
                    AccountingClassV1::Scheduled => Some((
                        budgets.protected_scheduled.clone(),
                        LedgerAdmissionRefusalV1::ProtectedScheduledExhausted,
                    )),
                    AccountingClassV1::TestMerge => Some((
                        budgets.protected_test_merge.clone(),
                        LedgerAdmissionRefusalV1::ProtectedTestMergeExhausted,
                    )),
                    _ => unreachable!(),
                };
                Ok(BudgetLimits {
                    utc_day: budgets.utc_day.clone(),
                    rolling_24h: budgets.rolling_24h.clone(),
                    per_case: per_case.clone(),
                    per_provider: per_provider.clone(),
                    per_trigger: Some(per_trigger.clone()),
                    class_pool,
                })
            }
            Self::ManualUnallocated {
                manual_admission_sha256,
                accounting_grant_sha256,
                budgets,
            } => {
                let AdmissionAuthorityV1::ManualAcknowledgement(bound) = authority else {
                    return Err("schedule ledger: manual policy has wrong authority".into());
                };
                if request.accounting_class != AccountingClassV1::Manual
                    || trigger != TriggerKindV1::ManualCompatibility
                    || &bound.manual_admission_sha256 != manual_admission_sha256
                    || !local_file::valid_sha256(accounting_grant_sha256)
                {
                    return Err("schedule ledger: manual policy bindings diverged".into());
                }
                validate_budget_policy(budgets)?;
                let per_case = find_named(&budgets.per_case, request.case_id)
                    .ok_or("schedule ledger: manual policy has no case cap")?;
                let per_provider = find_named(&budgets.per_provider, request.provider_family)
                    .ok_or("schedule ledger: manual policy has no provider cap")?;
                Ok(BudgetLimits {
                    utc_day: budgets.utc_day.clone(),
                    rolling_24h: budgets.rolling_24h.clone(),
                    per_case: per_case.clone(),
                    per_provider: per_provider.clone(),
                    // Standing grants intentionally do not authorize manual triggers. Manual
                    // accounting is bounded by its distinct class pool instead.
                    per_trigger: None,
                    class_pool: Some((
                        budgets.manual_unallocated.clone(),
                        LedgerAdmissionRefusalV1::ManualUnallocatedExhausted,
                    )),
                })
            }
        }
    }
}

fn utc_day_id(timestamp_ms: i64) -> Result<String, BoxError> {
    if timestamp_ms <= 0 {
        return Err("schedule ledger: UTC timestamp must be positive".into());
    }
    // Howard Hinnant's civil-from-days conversion, using Euclidean arithmetic for a canonical UTC
    // calendar date without locale, timezone, or libc state.
    let z = timestamp_ms.div_euclid(DAY_MILLIS) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    if !(1970..=9999).contains(&year) {
        return Err("schedule ledger: UTC timestamp is outside the supported calendar".into());
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

fn rolling_window_id(timestamp_ms: i64) -> Result<String, BoxError> {
    if timestamp_ms <= 0 {
        return Err("schedule ledger: rolling timestamp must be positive".into());
    }
    Ok(format!("rolling-24h-{timestamp_ms}"))
}

#[derive(Debug)]
pub(super) enum LedgerHeadroomError {
    Invalid(String),
    Refused(LedgerAdmissionRefusalV1),
}

impl std::fmt::Display for LedgerHeadroomError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Refused(reason) => write!(formatter, "schedule ledger: {reason:?}"),
        }
    }
}

impl std::error::Error for LedgerHeadroomError {}

fn invalid_headroom(error: impl std::fmt::Display) -> LedgerHeadroomError {
    LedgerHeadroomError::Invalid(error.to_string())
}

#[derive(Clone, Debug)]
struct StoredReservationV1 {
    record: LedgerReservationV1,
    record_sha256: String,
    reconciliation: Option<LedgerReconciliationV1>,
}

pub(super) struct FileCompatibilityLedger<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    reservations: BTreeMap<String, StoredReservationV1>,
    imports: BTreeMap<String, LegacyLedgerImportV1>,
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, BoxError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("schedule ledger: record exceeds the byte bound".into());
    }
    Ok(bytes)
}

fn read_owner_record(
    directory: &local_file::PinnedDirectory,
    name: &str,
) -> Result<Vec<u8>, BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    let file = directory.open_regular_file(OsStr::new(name), "schedule ledger record")?;
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > MAX_RECORD_BYTES
    {
        return Err(
            "schedule ledger: record is not a bounded owner-only mode-0600 regular file".into(),
        );
    }
    Ok(local_file::read_open_regular_file_bounded(
        &file,
        "schedule ledger record",
        MAX_RECORD_BYTES,
    )?
    .bytes)
}

fn append_record(
    directory: &local_file::PinnedDirectory,
    name: &str,
    bytes: &[u8],
) -> Result<LedgerAppendOutcomeV1, BoxError> {
    if directory
        .open_regular_file(OsStr::new(name), "existing ledger record")
        .is_ok()
    {
        if read_owner_record(directory, name)? == bytes {
            return Ok(LedgerAppendOutcomeV1::ExistingIdentical);
        }
        return Err("schedule ledger: create-new record name already has different bytes".into());
    }
    let mut file = directory.create_new_file(OsStr::new(name), 0o600, "schedule ledger record")?;
    // A failed write/sync is intentionally not unlinked. Recovery must see the partial record and
    // hold rather than accidentally manufacture headroom after an ambiguous durable effect.
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("schedule ledger: cannot persist record: {error}"))?;
    directory.sync()?;
    Ok(LedgerAppendOutcomeV1::Created)
}

fn expected_reservation_id(attempt_idempotency_key: &str) -> String {
    format!("ledger-{attempt_idempotency_key}")
}

fn validate_class_authority_trigger(value: &LedgerReservationV1) -> Result<(), BoxError> {
    let valid = matches!(
        (&value.authority, value.accounting_class, value.trigger),
        (
            AdmissionAuthorityV1::CharacterizationOnce(_),
            AccountingClassV1::Characterization,
            TriggerKindV1::ManualCharacterization
        ) | (
            AdmissionAuthorityV1::StandingGrant(_),
            AccountingClassV1::Scheduled,
            TriggerKindV1::Daily | TriggerKindV1::ScheduledMain
        ) | (
            AdmissionAuthorityV1::StandingGrant(_),
            AccountingClassV1::TestMerge,
            TriggerKindV1::TestMerge
        ) | (
            AdmissionAuthorityV1::ManualAcknowledgement(_),
            AccountingClassV1::Manual,
            TriggerKindV1::ManualCompatibility
        )
    );
    if !valid {
        return Err("schedule ledger: accounting class, authority, and trigger disagree".into());
    }
    Ok(())
}

fn validate_reservation(value: &LedgerReservationV1) -> Result<(), BoxError> {
    LedgerRecordV1::Reservation(value.clone()).validate()?;
    stable_id("case id", &value.case_id)?;
    stable_id("provider family", &value.provider_family)?;
    validate_class_authority_trigger(value)?;
    let expected_attempt = crate::compatibility_schedule_admission::attempt_idempotency_key(
        &value.admission_attempt,
        &value.repeat_nonce,
    )?;
    let expected_equivalent = crate::compatibility_schedule_admission::equivalent_work_key(
        &value.case_execution,
        value.evidence_purpose,
        &value.freshness_bucket,
    )?;
    if value.attempt_idempotency_key != expected_attempt
        || value.reservation_id != expected_reservation_id(&expected_attempt)
        || value.equivalent_work_key != expected_equivalent
        || value.utc_day_id != utc_day_id(value.reserved_at_ms)?
        || value.rolling_window_id != rolling_window_id(value.reserved_at_ms)?
    {
        return Err("schedule ledger: reservation derived identities diverged".into());
    }
    Ok(())
}

pub(super) fn prepared_reservation_sha256(value: &LedgerReservationV1) -> Result<String, BoxError> {
    validate_reservation(value)?;
    Ok(local_file::sha256_hex(&canonical_bytes(
        &LedgerRecordV1::Reservation(value.clone()),
    )?))
}

pub(super) fn validate_prepared_reservation_context(
    value: &LedgerReservationV1,
    context: &DerivedLedgerAdmissionContextV1,
) -> Result<(), BoxError> {
    validate_reservation(value)?;
    context.identities.validate()?;
    if value.attempt_idempotency_key != context.identities.attempt_idempotency_key
        || value.accounting_class != context.accounting_class
        || value.case_id != context.case_id
        || value.provider_family != context.provider_family
        || value.characterization_profile != context.identities.characterization_profile
        || value.case_execution != context.identities.case_execution.fingerprint
        || value.admission_attempt != context.identities.admission_attempt.fingerprint
        || value.authority != context.identities.admission_attempt.input.authority
        || value.equivalent_work_key != context.identities.equivalent_work_key
        || value.evidence_purpose != context.identities.evidence_purpose
        || value.freshness_bucket != context.identities.freshness_bucket
    {
        return Err(
            "schedule ledger: prepared reservation diverges from its derived context".into(),
        );
    }
    Ok(())
}

fn full_conservative_charge(caps: &EffectCapsV1) -> UsageChargeV1 {
    UsageChargeV1 {
        attempts: caps.attempts,
        tokens: caps.max_tokens,
        cost_microusd: caps.max_cost_microusd,
        elapsed_millis: caps.timeout_secs.saturating_mul(1_000),
    }
}

fn charge_within_caps(charge: &UsageChargeV1, caps: &EffectCapsV1) -> bool {
    charge.attempts == 1
        && charge.tokens <= caps.max_tokens
        && charge.cost_microusd <= caps.max_cost_microusd
        && charge.elapsed_millis <= caps.timeout_secs.saturating_mul(1_000)
}

fn validate_reconciliation(
    reservation: &StoredReservationV1,
    value: &LedgerReconciliationV1,
) -> Result<(), BoxError> {
    LedgerRecordV1::Reconciliation(value.clone()).validate()?;
    let reserved = &reservation.record;
    if value.reservation_id != reserved.reservation_id
        || value.reservation_sha256 != reservation.record_sha256
        || value.characterization_profile != reserved.characterization_profile
        || value.case_execution != reserved.case_execution
        || value.admission_attempt != reserved.admission_attempt
        || value.authority != reserved.authority
        || value.equivalent_work_key != reserved.equivalent_work_key
        || value.reconciled_at_ms < reserved.reserved_at_ms
    {
        return Err("schedule ledger: reconciliation does not bind its reservation".into());
    }
    match value.disposition {
        LedgerDispositionV1::ReleasedPreEffect if value.reason_code == "proved-pre-effect" => {}
        LedgerDispositionV1::ChargedTerminal
            if value.reason_code == "valid-terminal"
                && charge_within_caps(&value.charged_usage, &reserved.caps) => {}
        LedgerDispositionV1::ChargedConservative
            if matches!(
                value.reason_code.as_str(),
                "spawn-state-ambiguous"
                    | "prompt-acceptance-possible"
                    | "kill-or-crash"
                    | "missing-usage"
                    | "invalid-usage"
                    | "unknown-price"
                    | "unknown-currency"
                    | "evidence-unreconciled"
            ) && value.charged_usage == full_conservative_charge(&reserved.caps) => {}
        _ => {
            return Err("schedule ledger: reconciliation charge exceeds or understates caps".into())
        }
    }
    Ok(())
}

fn validate_legacy_import(value: &LegacyLedgerImportV1) -> Result<(), BoxError> {
    if value.schema_version != 1
        || !local_file::valid_sha256(&value.inventory_sha256)
        || !local_file::valid_sha256(&value.aggregate_sha256)
        || value.charged_usage.attempts == 0
        || value.charged_usage.attempts > 10_000
        || value.charged_usage.tokens > 1_000_000_000
        || value.charged_usage.cost_microusd > 10_000_000_000
        || value.charged_usage.time_secs > 31 * 24 * 60 * 60
        || value.observed_at_ms <= 0
        || value.rolling_expires_at_ms != value.observed_at_ms.saturating_add(DAY_MILLIS)
    {
        return Err("schedule ledger: legacy import is malformed".into());
    }
    stable_id("legacy import id", &value.import_id)?;
    stable_id("legacy case id", &value.case_id)?;
    stable_id("legacy provider family", &value.provider_family)?;
    #[derive(Serialize)]
    struct Identity<'a> {
        inventory_sha256: &'a str,
        aggregate_sha256: &'a str,
        kind: LegacyImportKindV1,
        case_id: &'a str,
        provider_family: &'a str,
        trigger: TriggerKindV1,
        accounting_class: AccountingClassV1,
        charged_usage: AggregateUsageV1,
        observed_at_ms: i64,
    }
    let expected = format!(
        "legacy-{}",
        ledger_hash(
            "legacy-import-id",
            &Identity {
                inventory_sha256: &value.inventory_sha256,
                aggregate_sha256: &value.aggregate_sha256,
                kind: value.kind,
                case_id: &value.case_id,
                provider_family: &value.provider_family,
                trigger: value.trigger,
                accounting_class: value.accounting_class,
                charged_usage: value.charged_usage,
                observed_at_ms: value.observed_at_ms,
            },
        )?
    );
    if value.import_id != expected {
        return Err("schedule ledger: legacy import id is not canonical".into());
    }
    Ok(())
}

impl LegacyLedgerImportV1 {
    pub(super) fn new(input: LegacyLedgerImportInputV1) -> Result<Self, BoxError> {
        let LegacyLedgerImportInputV1 {
            inventory_sha256,
            aggregate_sha256,
            kind,
            case_id,
            provider_family,
            trigger,
            accounting_class,
            charged_usage,
            observed_at_ms,
        } = input;
        #[derive(Serialize)]
        struct Identity<'a> {
            inventory_sha256: &'a str,
            aggregate_sha256: &'a str,
            kind: LegacyImportKindV1,
            case_id: &'a str,
            provider_family: &'a str,
            trigger: TriggerKindV1,
            accounting_class: AccountingClassV1,
            charged_usage: AggregateUsageV1,
            observed_at_ms: i64,
        }
        let import_id = format!(
            "legacy-{}",
            ledger_hash(
                "legacy-import-id",
                &Identity {
                    inventory_sha256: &inventory_sha256,
                    aggregate_sha256: &aggregate_sha256,
                    kind,
                    case_id: &case_id,
                    provider_family: &provider_family,
                    trigger,
                    accounting_class,
                    charged_usage,
                    observed_at_ms,
                },
            )?
        );
        let value = Self {
            schema_version: 1,
            import_id,
            inventory_sha256,
            aggregate_sha256,
            kind,
            case_id,
            provider_family,
            trigger,
            accounting_class,
            charged_usage,
            observed_at_ms,
            rolling_expires_at_ms: observed_at_ms.saturating_add(DAY_MILLIS),
        };
        validate_legacy_import(&value)?;
        Ok(value)
    }
}

impl<'lock> FileCompatibilityLedger<'lock> {
    pub(super) fn open<C: AdmissionStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<Self, BoxError> {
        let directory = capability.ledger_directory();
        if !directory.current_path_matches() {
            return Err("schedule ledger: retained ledger directory path changed".into());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "schedule ledger: non-UTF8 record name")?;
            names.push(name);
        }
        if names.len() > MAX_LEDGER_FILES || !directory.current_path_matches() {
            return Err("schedule ledger: record scan is unbounded or unstable".into());
        }
        names.sort();
        let mut reservations = BTreeMap::<String, StoredReservationV1>::new();
        let mut reconciliations = Vec::<LedgerReconciliationV1>::new();
        let mut imports = BTreeMap::new();
        for name in names {
            let bytes = read_owner_record(directory, &name)?;
            if let Some(id) = name.strip_suffix(".reservation.json") {
                let record: LedgerRecordV1 = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("schedule ledger: invalid reservation: {error}"))?;
                let LedgerRecordV1::Reservation(value) = record else {
                    return Err(
                        "schedule ledger: reservation filename has wrong record kind".into(),
                    );
                };
                if value.reservation_id != id
                    || canonical_bytes(&LedgerRecordV1::Reservation(value.clone()))? != bytes
                {
                    return Err(
                        "schedule ledger: reservation filename or canonical bytes diverged".into(),
                    );
                }
                validate_reservation(&value)?;
                let sha256 = local_file::sha256_hex(&bytes);
                if reservations
                    .insert(
                        id.to_owned(),
                        StoredReservationV1 {
                            record: value,
                            record_sha256: sha256,
                            reconciliation: None,
                        },
                    )
                    .is_some()
                {
                    return Err("schedule ledger: duplicate reservation".into());
                }
            } else if let Some(id) = name.strip_suffix(".reconciliation.json") {
                let record: LedgerRecordV1 = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("schedule ledger: invalid reconciliation: {error}"))?;
                let LedgerRecordV1::Reconciliation(value) = record else {
                    return Err(
                        "schedule ledger: reconciliation filename has wrong record kind".into(),
                    );
                };
                if value.reservation_id != id
                    || canonical_bytes(&LedgerRecordV1::Reconciliation(value.clone()))? != bytes
                {
                    return Err(
                        "schedule ledger: reconciliation filename or canonical bytes diverged"
                            .into(),
                    );
                }
                reconciliations.push(value);
            } else if let Some(id) = name.strip_suffix(".legacy.json") {
                let value: LegacyLedgerImportV1 = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("schedule ledger: invalid legacy import: {error}"))?;
                if value.import_id != id || canonical_bytes(&value)? != bytes {
                    return Err(
                        "schedule ledger: legacy filename or canonical bytes diverged".into(),
                    );
                }
                validate_legacy_import(&value)?;
                if imports.insert(id.to_owned(), value).is_some() {
                    return Err("schedule ledger: duplicate legacy import".into());
                }
            } else {
                return Err(format!("schedule ledger: unexpected state entry {name:?}").into());
            }
        }
        let mut attempt_keys = BTreeSet::new();
        for reservation in reservations.values() {
            if !attempt_keys.insert(reservation.record.attempt_idempotency_key.as_str()) {
                return Err("schedule ledger: duplicate attempt-idempotency key".into());
            }
        }
        for reconciliation in reconciliations {
            let reservation = reservations
                .get_mut(&reconciliation.reservation_id)
                .ok_or("schedule ledger: reconciliation has no reservation")?;
            validate_reconciliation(reservation, &reconciliation)?;
            if reservation.reconciliation.replace(reconciliation).is_some() {
                return Err("schedule ledger: duplicate reconciliation".into());
            }
        }
        Ok(Self {
            directory,
            reservations,
            imports,
        })
    }

    fn charge(reservation: &StoredReservationV1) -> AggregateUsageV1 {
        match &reservation.reconciliation {
            Some(value) => AggregateUsageV1::from_charge(&value.charged_usage),
            None => AggregateUsageV1::from_caps(&reservation.record.caps),
        }
    }

    fn current_usage<F>(&self, mut include: F) -> Result<AggregateUsageV1, BoxError>
    where
        F: FnMut(&str, &str, TriggerKindV1, AccountingClassV1, i64, i64) -> bool,
    {
        let mut total = AggregateUsageV1::default();
        for value in self.reservations.values() {
            let record = &value.record;
            if include(
                &record.case_id,
                &record.provider_family,
                record.trigger,
                record.accounting_class,
                record.reserved_at_ms,
                record.reserved_at_ms.saturating_add(DAY_MILLIS),
            ) {
                total = total.checked_add(Self::charge(value))?;
            }
        }
        for value in self.imports.values() {
            if include(
                &value.case_id,
                &value.provider_family,
                value.trigger,
                value.accounting_class,
                value.observed_at_ms,
                value.rolling_expires_at_ms,
            ) {
                total = total.checked_add(value.charged_usage)?;
            }
        }
        Ok(total)
    }

    fn request_record(
        request: &LedgerReservationRequestV1<'_>,
        reserved_at_ms: i64,
    ) -> Result<LedgerReservationV1, BoxError> {
        stable_id("case id", request.case_id)?;
        stable_id("provider family", request.provider_family)?;
        let policy_sha256 = policy_sha256(request.budget_authority)?;
        request.budget_authority.limits(request)?;
        let trigger = &request.identities.admission_attempt.input.trigger;
        let record = LedgerReservationV1 {
            schema_version: 1,
            reservation_id: expected_reservation_id(&request.identities.attempt_idempotency_key),
            attempt_idempotency_key: request.identities.attempt_idempotency_key.clone(),
            accounting_class: request.accounting_class,
            case_id: request.case_id.to_owned(),
            provider_family: request.provider_family.to_owned(),
            trigger: trigger.kind,
            accounting_policy_sha256: policy_sha256,
            characterization_profile: request.identities.characterization_profile.clone(),
            case_execution: request.identities.case_execution.fingerprint.clone(),
            admission_attempt: request.identities.admission_attempt.fingerprint.clone(),
            authority: request.identities.admission_attempt.input.authority.clone(),
            equivalent_work_key: request.identities.equivalent_work_key.clone(),
            evidence_purpose: request.identities.evidence_purpose,
            freshness_bucket: request.identities.freshness_bucket.clone(),
            repeat_nonce: trigger.repeat_nonce.clone(),
            caps: request.identities.case_execution.input.actual_caps.clone(),
            utc_day_id: utc_day_id(reserved_at_ms)?,
            rolling_window_id: rolling_window_id(reserved_at_ms)?,
            reserved_at_ms,
        };
        validate_reservation(&record)?;
        Ok(record)
    }

    fn existing_matches_request(
        existing: &LedgerReservationV1,
        request: &LedgerReservationRequestV1<'_>,
    ) -> Result<bool, BoxError> {
        let mut proposed = Self::request_record(request, existing.reserved_at_ms)?;
        proposed.utc_day_id = existing.utc_day_id.clone();
        proposed.rolling_window_id = existing.rolling_window_id.clone();
        Ok(&proposed == existing)
    }

    pub(super) fn check_headroom(
        &self,
        request: &LedgerReservationRequestV1<'_>,
        now_ms: i64,
    ) -> Result<(), LedgerHeadroomError> {
        let proposed = Self::request_record(request, now_ms).map_err(invalid_headroom)?;
        if let Some(existing) = self.reservations.get(&proposed.reservation_id) {
            return if Self::existing_matches_request(&existing.record, request)
                .map_err(invalid_headroom)?
            {
                Ok(())
            } else {
                Err(LedgerHeadroomError::Invalid(
                    "schedule ledger: idempotent reservation request changed".into(),
                ))
            };
        }
        let latest = self
            .reservations
            .values()
            .flat_map(|value| {
                std::iter::once(value.record.reserved_at_ms).chain(
                    value
                        .reconciliation
                        .as_ref()
                        .map(|item| item.reconciled_at_ms),
                )
            })
            .chain(self.imports.values().map(|value| value.observed_at_ms))
            .max()
            .unwrap_or(0);
        if latest > now_ms {
            return Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::ClockRollback,
            ));
        }
        let limits = request
            .budget_authority
            .limits(request)
            .map_err(invalid_headroom)?;
        let reserve = AggregateUsageV1::from_caps(&proposed.caps);
        let day = proposed.utc_day_id.as_str();
        let day_usage = self
            .current_usage(|_, _, _, _, admitted_at, _| {
                utc_day_id(admitted_at).ok().as_deref() == Some(day)
            })
            .map_err(invalid_headroom)?;
        if !day_usage
            .checked_add(reserve)
            .map_err(invalid_headroom)?
            .within(&limits.utc_day)
        {
            return Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::UtcDayExhausted,
            ));
        }
        let rolling_usage = self
            .current_usage(|_, _, _, _, _, rolling_expires_at| rolling_expires_at > now_ms)
            .map_err(invalid_headroom)?;
        if !rolling_usage
            .checked_add(reserve)
            .map_err(invalid_headroom)?
            .within(&limits.rolling_24h)
        {
            return Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::Rolling24hExhausted,
            ));
        }
        let case_usage = self
            .current_usage(|case, _, _, _, admitted_at, _| {
                case == request.case_id && utc_day_id(admitted_at).ok().as_deref() == Some(day)
            })
            .map_err(invalid_headroom)?;
        if !case_usage
            .checked_add(reserve)
            .map_err(invalid_headroom)?
            .within(&limits.per_case)
        {
            return Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::PerCaseExhausted,
            ));
        }
        let provider_usage = self
            .current_usage(|_, provider, _, _, admitted_at, _| {
                provider == request.provider_family
                    && utc_day_id(admitted_at).ok().as_deref() == Some(day)
            })
            .map_err(invalid_headroom)?;
        if !provider_usage
            .checked_add(reserve)
            .map_err(invalid_headroom)?
            .within(&limits.per_provider)
        {
            return Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::PerProviderExhausted,
            ));
        }
        if let Some(limit) = &limits.per_trigger {
            let trigger_usage = self
                .current_usage(|_, _, trigger, _, admitted_at, _| {
                    trigger == proposed.trigger
                        && utc_day_id(admitted_at).ok().as_deref() == Some(day)
                })
                .map_err(invalid_headroom)?;
            if !trigger_usage
                .checked_add(reserve)
                .map_err(invalid_headroom)?
                .within(limit)
            {
                return Err(LedgerHeadroomError::Refused(
                    LedgerAdmissionRefusalV1::PerTriggerExhausted,
                ));
            }
        }
        if let Some((limit, refusal)) = &limits.class_pool {
            let class_usage = self
                .current_usage(|_, _, _, class, admitted_at, _| {
                    class == proposed.accounting_class
                        && utc_day_id(admitted_at).ok().as_deref() == Some(day)
                })
                .map_err(invalid_headroom)?;
            if !class_usage
                .checked_add(reserve)
                .map_err(invalid_headroom)?
                .within(limit)
            {
                return Err(LedgerHeadroomError::Refused(*refusal));
            }
        }
        Ok(())
    }

    pub(super) fn reserve(
        &mut self,
        request: &LedgerReservationRequestV1<'_>,
        reserved_at_ms: i64,
    ) -> Result<(LedgerAppendOutcomeV1, LedgerReservationV1), BoxError> {
        let record = self.prepare_reservation(request, reserved_at_ms)?;
        self.commit_prepared_reservation(record)
    }

    pub(super) fn prepare_reservation(
        &self,
        request: &LedgerReservationRequestV1<'_>,
        reserved_at_ms: i64,
    ) -> Result<LedgerReservationV1, BoxError> {
        self.check_headroom(request, reserved_at_ms)
            .map_err(|error| -> BoxError { error.to_string().into() })?;
        let proposed = Self::request_record(request, reserved_at_ms)?;
        if let Some(existing) = self.reservations.get(&proposed.reservation_id) {
            return Ok(existing.record.clone());
        }
        Ok(proposed)
    }

    pub(super) fn commit_prepared_reservation(
        &mut self,
        record: LedgerReservationV1,
    ) -> Result<(LedgerAppendOutcomeV1, LedgerReservationV1), BoxError> {
        validate_reservation(&record)?;
        if let Some(existing) = self.reservations.get(&record.reservation_id) {
            if existing.record == record {
                return Ok((
                    LedgerAppendOutcomeV1::ExistingIdentical,
                    existing.record.clone(),
                ));
            }
            return Err("schedule ledger: attempt id was rebound".into());
        }
        if self.reservations.values().any(|existing| {
            existing.record.attempt_idempotency_key == record.attempt_idempotency_key
        }) {
            return Err("schedule ledger: prepared attempt-idempotency key was rebound".into());
        }
        let wrapped = LedgerRecordV1::Reservation(record.clone());
        let bytes = canonical_bytes(&wrapped)?;
        let name = format!("{}.reservation.json", record.reservation_id);
        let outcome = append_record(self.directory, &name, &bytes)?;
        let stored = StoredReservationV1 {
            record: record.clone(),
            record_sha256: local_file::sha256_hex(&bytes),
            reconciliation: None,
        };
        self.reservations
            .insert(record.reservation_id.clone(), stored);
        Ok((outcome, record))
    }

    pub(super) fn reconcile(
        &mut self,
        reservation_id: &str,
        decision: ReconciliationDecisionV1,
    ) -> Result<(LedgerAppendOutcomeV1, LedgerReconciliationV1), BoxError> {
        let stored = self
            .reservations
            .get(reservation_id)
            .ok_or("schedule ledger: reconciliation reservation is absent")?;
        let (terminal_evidence_sha256, disposition, reason_code, charged_usage, prompt, at) =
            match decision {
                ReconciliationDecisionV1::ProvedPreEffect {
                    evidence_sha256,
                    reconciled_at_ms,
                } => (
                    evidence_sha256,
                    LedgerDispositionV1::ReleasedPreEffect,
                    "proved-pre-effect".to_owned(),
                    UsageChargeV1 {
                        attempts: 0,
                        tokens: 0,
                        cost_microusd: 0,
                        elapsed_millis: 0,
                    },
                    false,
                    reconciled_at_ms,
                ),
                ReconciliationDecisionV1::ValidTerminal {
                    evidence_sha256,
                    usage,
                    prompt_was_accepted,
                    reconciled_at_ms,
                } => (
                    evidence_sha256,
                    LedgerDispositionV1::ChargedTerminal,
                    "valid-terminal".to_owned(),
                    usage,
                    prompt_was_accepted,
                    reconciled_at_ms,
                ),
                ReconciliationDecisionV1::Conservative {
                    evidence_sha256,
                    reason,
                    prompt_may_have_been_accepted,
                    reconciled_at_ms,
                } => (
                    evidence_sha256,
                    LedgerDispositionV1::ChargedConservative,
                    match reason {
                        ConservativeChargeReasonV1::SpawnStateAmbiguous => "spawn-state-ambiguous",
                        ConservativeChargeReasonV1::PromptAcceptancePossible => {
                            "prompt-acceptance-possible"
                        }
                        ConservativeChargeReasonV1::KillOrCrash => "kill-or-crash",
                        ConservativeChargeReasonV1::MissingUsage => "missing-usage",
                        ConservativeChargeReasonV1::InvalidUsage => "invalid-usage",
                        ConservativeChargeReasonV1::UnknownPrice => "unknown-price",
                        ConservativeChargeReasonV1::UnknownCurrency => "unknown-currency",
                        ConservativeChargeReasonV1::EvidenceUnreconciled => "evidence-unreconciled",
                    }
                    .to_owned(),
                    full_conservative_charge(&stored.record.caps),
                    prompt_may_have_been_accepted,
                    reconciled_at_ms,
                ),
            };
        let value = LedgerReconciliationV1 {
            schema_version: 1,
            reservation_id: stored.record.reservation_id.clone(),
            reservation_sha256: stored.record_sha256.clone(),
            characterization_profile: stored.record.characterization_profile.clone(),
            case_execution: stored.record.case_execution.clone(),
            admission_attempt: stored.record.admission_attempt.clone(),
            authority: stored.record.authority.clone(),
            equivalent_work_key: stored.record.equivalent_work_key.clone(),
            terminal_evidence_sha256,
            disposition,
            reason_code,
            charged_usage,
            prompt_may_have_been_accepted: prompt,
            reconciled_at_ms: at,
        };
        validate_reconciliation(stored, &value)?;
        if let Some(existing) = &stored.reconciliation {
            return if existing == &value {
                Ok((LedgerAppendOutcomeV1::ExistingIdentical, existing.clone()))
            } else {
                Err("schedule ledger: reconciliation was already committed differently".into())
            };
        }
        let wrapped = LedgerRecordV1::Reconciliation(value.clone());
        let bytes = canonical_bytes(&wrapped)?;
        let name = format!("{}.reconciliation.json", value.reservation_id);
        let outcome = append_record(self.directory, &name, &bytes)?;
        self.reservations
            .get_mut(reservation_id)
            .expect("reservation was checked above")
            .reconciliation = Some(value.clone());
        Ok((outcome, value))
    }

    pub(super) fn import_legacy(
        &mut self,
        value: LegacyLedgerImportV1,
    ) -> Result<LedgerAppendOutcomeV1, BoxError> {
        validate_legacy_import(&value)?;
        if let Some(existing) = self.imports.get(&value.import_id) {
            return if existing == &value {
                Ok(LedgerAppendOutcomeV1::ExistingIdentical)
            } else {
                Err("schedule ledger: legacy import was rebound".into())
            };
        }
        let bytes = canonical_bytes(&value)?;
        let name = format!("{}.legacy.json", value.import_id);
        let outcome = append_record(self.directory, &name, &bytes)?;
        self.imports.insert(value.import_id.clone(), value);
        Ok(outcome)
    }

    pub(super) fn may_admit_regenerated_successor(&self, reservation_id: &str) -> bool {
        self.reservations
            .get(reservation_id)
            .and_then(|value| value.reconciliation.as_ref())
            .is_some_and(|value| value.disposition == LedgerDispositionV1::ReleasedPreEffect)
    }

    pub(super) fn legacy_import_ids(&self) -> BTreeSet<String> {
        self.imports.keys().cloned().collect()
    }

    #[cfg(test)]
    fn total_charge(&self) -> Result<AggregateUsageV1, BoxError> {
        self.current_usage(|_, _, _, _, _, _| true)
    }

    #[cfg(test)]
    fn reconciliation(&self, id: &str) -> Option<&LedgerReconciliationV1> {
        self.reservations
            .get(id)
            .and_then(|value| value.reconciliation.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_schedule::EvidencePurposeV1;
    use crate::compatibility_schedule_schema::{
        seal_admission_attempt_fingerprint, seal_case_execution_fingerprint,
        AdmissionAttemptFingerprintInputV1, AdmissionTriggerIdentityV1, CandidateBinaryIdentityV1,
        CaseExecutionFingerprintInputV1, CharacterizationOnceAuthorityV1, EffectiveIdentityV1,
        ExactExecutionBindingsV1, ExactExecutionTargetV1, FingerprintV1, GitObjectAlgorithmV1,
        GitObjectIdV1, ManualAcknowledgementAuthorityV1, NamedBudgetCapsV1, OptionalGitObjectIdV1,
        OptionalSha256V1, OptionalStableIdV1, OptionalTextV1, StandingGrantAuthorityV1,
        TriggerBudgetCapsV1, TriggerSourceV1,
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

    fn policy() -> GrantBudgetPolicyV1 {
        GrantBudgetPolicyV1 {
            per_case: ["case-1", "case-2", "case-3"]
                .into_iter()
                .map(|id| NamedBudgetCapsV1 {
                    id: id.into(),
                    caps: aggregate(3),
                })
                .collect(),
            per_trigger_pool: [
                TriggerKindV1::Daily,
                TriggerKindV1::ScheduledMain,
                TriggerKindV1::TestMerge,
            ]
            .into_iter()
            .map(|trigger| TriggerBudgetCapsV1 {
                trigger,
                caps: aggregate(3),
            })
            .collect(),
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

    fn standing_authority() -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::StandingGrant(StandingGrantAuthorityV1 {
            grant_id: "grant-1".into(),
            generation: 1,
            grant_sha256: digest('a'),
            characterization_id: "characterization-1".into(),
            characterization_sha256: digest('b'),
        })
    }

    fn manual_authority(seed: &str) -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::ManualAcknowledgement(ManualAcknowledgementAuthorityV1 {
            manual_admission_sha256: digest('c'),
            request_nonce: seed.into(),
        })
    }

    fn characterization_authority() -> AdmissionAuthorityV1 {
        AdmissionAuthorityV1::CharacterizationOnce(CharacterizationOnceAuthorityV1 {
            batch_authorization_id: "authorization-1".into(),
            batch_authorization_sha256: digest('d'),
            entry_id: "entry-1".into(),
            generation: 1,
            entry_sha256: digest('e'),
            consumption_nonce: "consumption-1".into(),
        })
    }

    fn trigger(kind: TriggerKindV1, seed: &str) -> AdmissionTriggerIdentityV1 {
        AdmissionTriggerIdentityV1 {
            source: match kind {
                TriggerKindV1::ManualCharacterization => TriggerSourceV1::ManualCharacterizationCli,
                TriggerKindV1::ManualCompatibility => TriggerSourceV1::ManualCompatibilityCli,
                TriggerKindV1::Daily => TriggerSourceV1::DailyLaunchd,
                TriggerKindV1::ScheduledMain => TriggerSourceV1::ScheduledMainCoalescer,
                TriggerKindV1::TestMerge => TriggerSourceV1::TestMergeWatcher,
            },
            kind,
            request_id: format!("request-{seed}"),
            window_id: format!("window-{seed}"),
            attempt_id: format!("attempt-{seed}"),
            repeat_nonce: OptionalStableIdV1::Absent,
        }
    }

    fn identities(
        authority: AdmissionAuthorityV1,
        kind: TriggerKindV1,
        seed: char,
    ) -> DerivedAdmissionIdentitiesV1 {
        let profile = fingerprint('1');
        let execution = seal_case_execution_fingerprint(CaseExecutionFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: profile.clone(),
            target: ExactExecutionTargetV1::RepositorySnapshot {
                repository: "shoedog/a2acp".into(),
                head_oid: GitObjectIdV1 {
                    algorithm: GitObjectAlgorithmV1::Sha1,
                    hex: seed.to_string().repeat(40),
                },
                tree_oid: GitObjectIdV1 {
                    algorithm: GitObjectAlgorithmV1::Sha1,
                    hex: "2".repeat(40),
                },
                range_start_exclusive: OptionalGitObjectIdV1::Absent,
            },
            candidate: CandidateBinaryIdentityV1 {
                sha256: digest(seed),
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
        })
        .unwrap();
        let trigger = trigger(kind, &seed.to_string());
        let admission = seal_admission_attempt_fingerprint(AdmissionAttemptFingerprintInputV1 {
            schema_version: 1,
            characterization_profile: profile.clone(),
            case_execution: execution.fingerprint.clone(),
            authority,
            trigger,
        })
        .unwrap();
        let evidence_purpose = match kind {
            TriggerKindV1::ManualCharacterization => EvidencePurposeV1::Characterization,
            TriggerKindV1::ManualCompatibility => EvidencePurposeV1::ManualDiagnostic,
            _ => EvidencePurposeV1::ProviderPathAdvisory,
        };
        let freshness_bucket = "policy-window-1".to_owned();
        DerivedAdmissionIdentitiesV1 {
            characterization_profile: profile,
            equivalent_work_key: crate::compatibility_schedule_admission::equivalent_work_key(
                &execution.fingerprint,
                evidence_purpose,
                &freshness_bucket,
            )
            .unwrap(),
            attempt_idempotency_key:
                crate::compatibility_schedule_admission::attempt_idempotency_key(
                    &admission.fingerprint,
                    &admission.input.trigger.repeat_nonce,
                )
                .unwrap(),
            case_execution: execution,
            admission_attempt: admission,
            evidence_purpose,
            freshness_bucket,
        }
    }

    fn standing_policy() -> LedgerBudgetAuthorityV1 {
        LedgerBudgetAuthorityV1::StandingGrant {
            grant_sha256: digest('a'),
            budgets: policy(),
        }
    }

    fn manual_policy() -> LedgerBudgetAuthorityV1 {
        LedgerBudgetAuthorityV1::ManualUnallocated {
            manual_admission_sha256: digest('c'),
            accounting_grant_sha256: digest('a'),
            budgets: policy(),
        }
    }

    fn characterization_policy() -> LedgerBudgetAuthorityV1 {
        LedgerBudgetAuthorityV1::CharacterizationOnce {
            entry_sha256: digest('e'),
            case_id: "case-1".into(),
            provider_family: "provider-1".into(),
            caps: caps(),
        }
    }

    fn request<'a>(
        ids: &'a DerivedAdmissionIdentitiesV1,
        class: AccountingClassV1,
        case_id: &'a str,
        policy: &'a LedgerBudgetAuthorityV1,
    ) -> LedgerReservationRequestV1<'a> {
        LedgerReservationRequestV1 {
            identities: ids,
            accounting_class: class,
            case_id,
            provider_family: "provider-1",
            budget_authority: policy,
        }
    }

    fn state_root() -> (tempfile::TempDir, SchedulerStateRoot) {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        (root, state)
    }

    #[test]
    fn utc_day_is_canonical_and_midnight_does_not_move_the_charge() {
        assert_eq!(utc_day_id(1).unwrap(), "1970-01-01");
        assert_eq!(utc_day_id(DAY_MILLIS).unwrap(), "1970-01-02");
        assert_eq!(utc_day_id(951_782_400_000).unwrap(), "2000-02-29");

        let (_root, state) = state_root();
        let owner = state.try_owner_admission("test:midnight").unwrap();
        let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
        let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let budget = standing_policy();
        let scheduled_request = request(&ids, AccountingClassV1::Scheduled, "case-1", &budget);
        let (_, reserved) = ledger
            .reserve(&scheduled_request, DAY_MILLIS - 1_000)
            .unwrap();
        ledger
            .reconcile(
                &reserved.reservation_id,
                ReconciliationDecisionV1::ValidTerminal {
                    evidence_sha256: digest('f'),
                    usage: UsageChargeV1 {
                        attempts: 1,
                        tokens: 10,
                        cost_microusd: 10,
                        elapsed_millis: 10,
                    },
                    prompt_was_accepted: true,
                    reconciled_at_ms: DAY_MILLIS + 1_000,
                },
            )
            .unwrap();
        assert_eq!(reserved.utc_day_id, "1970-01-01");

        // A one-shot policy isolates the rolling boundary without weakening protected-pool
        // validation on the standing grant.
        let characterization = identities(
            characterization_authority(),
            TriggerKindV1::ManualCharacterization,
            'e',
        );
        let char_policy = characterization_policy();
        let char_request = request(
            &characterization,
            AccountingClassV1::Characterization,
            "case-1",
            &char_policy,
        );
        assert!(matches!(
            ledger.check_headroom(&char_request, DAY_MILLIS + 2_000),
            Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::Rolling24hExhausted
                    | LedgerAdmissionRefusalV1::UtcDayExhausted
            ))
        ));
    }

    #[test]
    fn reservation_restart_and_reconciliation_crash_points_are_conservative_and_idempotent() {
        let (_root, state) = state_root();
        let owner = state.try_owner_admission("test:crash").unwrap();
        let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let budget = standing_policy();
        let original_request = request(&ids, AccountingClassV1::Scheduled, "case-1", &budget);
        let reservation = {
            let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
            let (outcome, reservation) = ledger.reserve(&original_request, 1_000).unwrap();
            assert_eq!(outcome, LedgerAppendOutcomeV1::Created);
            assert_eq!(
                ledger.total_charge().unwrap(),
                AggregateUsageV1::from_caps(&caps())
            );
            reservation
        };
        let mut recovered = FileCompatibilityLedger::open(&owner).unwrap();
        assert_eq!(
            recovered.total_charge().unwrap(),
            AggregateUsageV1::from_caps(&caps())
        );
        assert_eq!(
            recovered.reserve(&original_request, 2_000).unwrap().0,
            LedgerAppendOutcomeV1::ExistingIdentical
        );
        let changed_request = request(&ids, AccountingClassV1::Scheduled, "case-2", &budget);
        assert!(recovered.reserve(&changed_request, 2_000).is_err());
        let decision = ReconciliationDecisionV1::Conservative {
            evidence_sha256: digest('f'),
            reason: ConservativeChargeReasonV1::MissingUsage,
            prompt_may_have_been_accepted: true,
            reconciled_at_ms: 3_000,
        };
        assert_eq!(
            recovered
                .reconcile(&reservation.reservation_id, decision.clone())
                .unwrap()
                .0,
            LedgerAppendOutcomeV1::Created
        );
        assert_eq!(
            recovered
                .reconcile(&reservation.reservation_id, decision)
                .unwrap()
                .0,
            LedgerAppendOutcomeV1::ExistingIdentical
        );
        assert_eq!(
            recovered
                .reconciliation(&reservation.reservation_id)
                .unwrap()
                .charged_usage,
            full_conservative_charge(&caps())
        );
        assert!(!recovered.may_admit_regenerated_successor(&reservation.reservation_id));
        drop(recovered);
        assert_eq!(
            FileCompatibilityLedger::open(&owner)
                .unwrap()
                .total_charge()
                .unwrap(),
            AggregateUsageV1::from_caps(&caps())
        );
    }

    #[test]
    fn proved_pre_effect_release_is_the_only_regeneration_safe_path() {
        let (_root, state) = state_root();
        let owner = state.try_owner_admission("test:release").unwrap();
        let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
        let budget = standing_policy();
        let first = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let first_request = request(&first, AccountingClassV1::Scheduled, "case-1", &budget);
        let (_, reserved) = ledger.reserve(&first_request, 1_000).unwrap();
        ledger
            .reconcile(
                &reserved.reservation_id,
                ReconciliationDecisionV1::ProvedPreEffect {
                    evidence_sha256: digest('f'),
                    reconciled_at_ms: 2_000,
                },
            )
            .unwrap();
        assert!(ledger.may_admit_regenerated_successor(&reserved.reservation_id));
        assert_eq!(ledger.total_charge().unwrap(), AggregateUsageV1::default());

        let second = identities(standing_authority(), TriggerKindV1::Daily, 'd');
        let second_request = request(&second, AccountingClassV1::Scheduled, "case-2", &budget);
        let (_, reserved) = ledger.reserve(&second_request, 3_000).unwrap();
        ledger
            .reconcile(
                &reserved.reservation_id,
                ReconciliationDecisionV1::Conservative {
                    evidence_sha256: digest('e'),
                    reason: ConservativeChargeReasonV1::PromptAcceptancePossible,
                    prompt_may_have_been_accepted: true,
                    reconciled_at_ms: 4_000,
                },
            )
            .unwrap();
        assert!(!ledger.may_admit_regenerated_successor(&reserved.reservation_id));
    }

    #[test]
    fn accounting_classes_use_disjoint_pools_and_manual_never_borrows() {
        let (_root, state) = state_root();
        let owner = state.try_owner_admission("test:pools").unwrap();
        let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
        let budget = standing_policy();
        let scheduled = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        ledger
            .reserve(
                &request(&scheduled, AccountingClassV1::Scheduled, "case-1", &budget),
                1_000,
            )
            .unwrap();
        let scheduled_two = identities(standing_authority(), TriggerKindV1::Daily, 'd');
        assert!(matches!(
            ledger.check_headroom(
                &request(
                    &scheduled_two,
                    AccountingClassV1::Scheduled,
                    "case-2",
                    &budget,
                ),
                2_000,
            ),
            Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::ProtectedScheduledExhausted
            ))
        ));

        let test_merge = identities(standing_authority(), TriggerKindV1::TestMerge, 'e');
        ledger
            .reserve(
                &request(&test_merge, AccountingClassV1::TestMerge, "case-2", &budget),
                3_000,
            )
            .unwrap();
        let manual_ids = identities(
            manual_authority("manual-1"),
            TriggerKindV1::ManualCompatibility,
            'f',
        );
        let mut manual_budget = manual_policy();
        let LedgerBudgetAuthorityV1::ManualUnallocated { budgets, .. } = &mut manual_budget else {
            unreachable!()
        };
        // Make the protected scheduled allocation deliberately larger than manual headroom so a
        // wrong-pool implementation would admit the second manual request.
        budgets.protected_scheduled = aggregate(2);
        budgets.utc_day = aggregate(4);
        budgets.rolling_24h = aggregate(4);
        budgets.per_provider[0].caps = aggregate(4);
        for item in &mut budgets.per_case {
            item.caps = aggregate(4);
        }
        ledger
            .reserve(
                &request(
                    &manual_ids,
                    AccountingClassV1::Manual,
                    "case-3",
                    &manual_budget,
                ),
                4_000,
            )
            .unwrap();

        let released_scheduled = ledger
            .reservations
            .values()
            .find(|value| value.record.accounting_class == AccountingClassV1::Scheduled)
            .unwrap()
            .record
            .reservation_id
            .clone();
        ledger
            .reconcile(
                &released_scheduled,
                ReconciliationDecisionV1::ProvedPreEffect {
                    evidence_sha256: digest('1'),
                    reconciled_at_ms: 5_000,
                },
            )
            .unwrap();
        let manual_two = identities(
            manual_authority("manual-2"),
            TriggerKindV1::ManualCompatibility,
            '1',
        );
        assert!(matches!(
            ledger.check_headroom(
                &request(
                    &manual_two,
                    AccountingClassV1::Manual,
                    "case-1",
                    &manual_budget,
                ),
                6_000,
            ),
            Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::ManualUnallocatedExhausted
            ))
        ));
    }

    #[test]
    fn legacy_imports_are_append_only_and_unknown_windows_consume_headroom() {
        let (_root, state) = state_root();
        let owner = state.try_owner_admission("test:legacy").unwrap();
        let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
        let import = LegacyLedgerImportV1::new(LegacyLedgerImportInputV1 {
            inventory_sha256: digest('1'),
            aggregate_sha256: digest('2'),
            kind: LegacyImportKindV1::UnknownInitialRollingWindow,
            case_id: "legacy-case".into(),
            provider_family: "legacy-provider".into(),
            trigger: TriggerKindV1::Daily,
            accounting_class: AccountingClassV1::Characterization,
            charged_usage: AggregateUsageV1 {
                attempts: 3,
                tokens: 300,
                cost_microusd: 3_000,
                time_secs: 90,
            },
            observed_at_ms: DAY_MILLIS - 1_000,
        })
        .unwrap();
        assert_eq!(
            ledger.import_legacy(import.clone()).unwrap(),
            LedgerAppendOutcomeV1::Created
        );
        assert_eq!(
            ledger.import_legacy(import).unwrap(),
            LedgerAppendOutcomeV1::ExistingIdentical
        );
        let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let policy = LedgerBudgetAuthorityV1::StandingGrant {
            grant_sha256: digest('a'),
            budgets: policy(),
        };
        assert!(matches!(
            ledger.check_headroom(
                &request(&ids, AccountingClassV1::Scheduled, "case-2", &policy),
                DAY_MILLIS + 1_000,
            ),
            Err(LedgerHeadroomError::Refused(
                LedgerAdmissionRefusalV1::Rolling24hExhausted
            ))
        ));
        assert!(ledger
            .check_headroom(
                &request(
                    &ids,
                    AccountingClassV1::Scheduled,
                    "case-2",
                    &standing_policy()
                ),
                2 * DAY_MILLIS,
            )
            .is_ok());
    }

    #[test]
    fn partial_or_rebound_records_hold_instead_of_creating_headroom() {
        let (root, state) = state_root();
        let owner = state.try_owner_admission("test:partial").unwrap();
        std::fs::write(root.path().join("ledger/torn.reservation.json"), b"{\n").unwrap();
        std::fs::set_permissions(
            root.path().join("ledger/torn.reservation.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(FileCompatibilityLedger::open(&owner).is_err());
    }

    #[test]
    fn wrong_authority_class_and_invalid_terminal_usage_fail_before_commit() {
        let (_root, state) = state_root();
        let owner = state.try_owner_admission("test:negative").unwrap();
        let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
        let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let policy = standing_policy();
        assert!(ledger
            .reserve(
                &request(&ids, AccountingClassV1::Manual, "case-1", &policy),
                1_000,
            )
            .is_err());
        let (_, reservation) = ledger
            .reserve(
                &request(&ids, AccountingClassV1::Scheduled, "case-1", &policy),
                1_000,
            )
            .unwrap();
        assert!(ledger
            .reconcile(
                &reservation.reservation_id,
                ReconciliationDecisionV1::ValidTerminal {
                    evidence_sha256: digest('f'),
                    usage: UsageChargeV1 {
                        attempts: 1,
                        tokens: caps().max_tokens + 1,
                        cost_microusd: 0,
                        elapsed_millis: 1,
                    },
                    prompt_was_accepted: true,
                    reconciled_at_ms: 2_000,
                },
            )
            .is_err());
        assert!(ledger.reconciliation(&reservation.reservation_id).is_none());
    }

    #[test]
    fn ledger_request_dimensions_come_from_the_rederived_source_context() {
        let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let context = DerivedLedgerAdmissionContextV1 {
            identities: ids,
            accounting_class: AccountingClassV1::Scheduled,
            case_id: "case-1".into(),
            provider_family: "provider-1".into(),
        };
        let budget = standing_policy();
        let request = LedgerReservationRequestV1::from_derived_context(&context, &budget);
        assert_eq!(request.case_id, "case-1");
        assert_eq!(request.provider_family, "provider-1");
        assert_eq!(request.accounting_class, AccountingClassV1::Scheduled);
    }

    #[test]
    fn every_ambiguous_terminal_reason_keeps_all_conservative_dimensions() {
        let reasons = [
            ConservativeChargeReasonV1::SpawnStateAmbiguous,
            ConservativeChargeReasonV1::PromptAcceptancePossible,
            ConservativeChargeReasonV1::KillOrCrash,
            ConservativeChargeReasonV1::MissingUsage,
            ConservativeChargeReasonV1::InvalidUsage,
            ConservativeChargeReasonV1::UnknownPrice,
            ConservativeChargeReasonV1::UnknownCurrency,
            ConservativeChargeReasonV1::EvidenceUnreconciled,
        ];
        for (index, reason) in reasons.into_iter().enumerate() {
            let (_root, state) = state_root();
            let owner = state.try_owner_admission("test:conservative").unwrap();
            let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
            let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
            let budget = standing_policy();
            let (_, reservation) = ledger
                .reserve(
                    &request(&ids, AccountingClassV1::Scheduled, "case-1", &budget),
                    1_000,
                )
                .unwrap();
            ledger
                .reconcile(
                    &reservation.reservation_id,
                    ReconciliationDecisionV1::Conservative {
                        evidence_sha256: digest(char::from_digit((index + 1) as u32, 16).unwrap()),
                        reason,
                        prompt_may_have_been_accepted: true,
                        reconciled_at_ms: 2_000,
                    },
                )
                .unwrap();
            assert_eq!(
                ledger.total_charge().unwrap(),
                AggregateUsageV1::from_caps(&caps())
            );
        }
    }

    #[test]
    fn valid_terminal_can_reconcile_downward_but_subscription_attempt_is_retained() {
        let (_root, state) = state_root();
        let owner = state.try_owner_admission("test:terminal").unwrap();
        let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
        let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let budget = standing_policy();
        let (_, reservation) = ledger
            .reserve(
                &request(&ids, AccountingClassV1::Scheduled, "case-1", &budget),
                1_000,
            )
            .unwrap();
        ledger
            .reconcile(
                &reservation.reservation_id,
                ReconciliationDecisionV1::ValidTerminal {
                    evidence_sha256: digest('f'),
                    usage: UsageChargeV1 {
                        attempts: 1,
                        tokens: 0,
                        cost_microusd: 0,
                        elapsed_millis: 0,
                    },
                    prompt_was_accepted: true,
                    reconciled_at_ms: 2_000,
                },
            )
            .unwrap();
        assert_eq!(
            ledger.total_charge().unwrap(),
            AggregateUsageV1 {
                attempts: 1,
                tokens: 0,
                cost_microusd: 0,
                time_secs: 0,
            }
        );
    }

    #[test]
    fn case_provider_and_trigger_caps_each_refuse_before_the_shared_class_pool() {
        #[derive(Clone, Copy)]
        enum Dimension {
            Case,
            Provider,
            Trigger,
        }
        for dimension in [Dimension::Case, Dimension::Provider, Dimension::Trigger] {
            let (_root, state) = state_root();
            let owner = state.try_owner_admission("test:dimension").unwrap();
            let mut ledger = FileCompatibilityLedger::open(&owner).unwrap();
            let mut budgets = policy();
            match dimension {
                Dimension::Case => {
                    budgets
                        .per_case
                        .iter_mut()
                        .find(|entry| entry.id == "case-1")
                        .unwrap()
                        .caps = aggregate(1)
                }
                Dimension::Provider => budgets.per_provider[0].caps = aggregate(1),
                Dimension::Trigger => {
                    budgets
                        .per_trigger_pool
                        .iter_mut()
                        .find(|entry| entry.trigger == TriggerKindV1::Daily)
                        .unwrap()
                        .caps = aggregate(1)
                }
            }
            let budget = LedgerBudgetAuthorityV1::StandingGrant {
                grant_sha256: digest('a'),
                budgets,
            };
            let first = identities(standing_authority(), TriggerKindV1::Daily, 'c');
            let first_case = "case-1";
            ledger
                .reserve(
                    &request(&first, AccountingClassV1::Scheduled, first_case, &budget),
                    1_000,
                )
                .unwrap();
            let second = identities(standing_authority(), TriggerKindV1::Daily, 'd');
            let second_case = match dimension {
                Dimension::Case => "case-1",
                Dimension::Provider | Dimension::Trigger => "case-2",
            };
            let expected = match dimension {
                Dimension::Case => LedgerAdmissionRefusalV1::PerCaseExhausted,
                Dimension::Provider => LedgerAdmissionRefusalV1::PerProviderExhausted,
                Dimension::Trigger => LedgerAdmissionRefusalV1::PerTriggerExhausted,
            };
            assert!(matches!(
                ledger.check_headroom(
                    &request(
                        &second,
                        AccountingClassV1::Scheduled,
                        second_case,
                        &budget,
                    ),
                    2_000,
                ),
                Err(LedgerHeadroomError::Refused(actual)) if actual == expected
            ));
        }
    }

    #[test]
    fn torn_reconciliation_holds_on_restart() {
        let (root, state) = state_root();
        let owner = state
            .try_owner_admission("test:torn-reconciliation")
            .unwrap();
        let ids = identities(standing_authority(), TriggerKindV1::Daily, 'c');
        let budget = standing_policy();
        let reservation = FileCompatibilityLedger::open(&owner)
            .unwrap()
            .reserve(
                &request(&ids, AccountingClassV1::Scheduled, "case-1", &budget),
                1_000,
            )
            .unwrap()
            .1;
        let path = root.path().join("ledger").join(format!(
            "{}.reconciliation.json",
            reservation.reservation_id
        ));
        std::fs::write(&path, b"{\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(FileCompatibilityLedger::open(&owner).is_err());
    }
}
