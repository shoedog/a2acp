//! Zero-provider-effect initial/final preflight fences for R3d2.
//!
//! The fence exposes only a closed local-proof interface. It has no provider, model-discovery,
//! registry-effect, image-pull, or agent-spawn capability. The same ordered checklist runs at the
//! initial and final fences, and action-time directory authority is descriptor-pinned separately.

#![allow(dead_code)] // R3d2e wires the production local-proof adapter after this checkpoint.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compatibility_process_group::{self, ProcessIdentityV1};
use crate::{local_file, BoxError};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PreflightFenceV1 {
    Initial,
    Final,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum PreflightCheckV1 {
    OwnerAndArchitecture,
    EffectAuthorityAndPolicy,
    CandidateBinary,
    GeneratedConfig,
    ProductionManifest,
    FloatingRecipe,
    ScheduledRegistry,
    ControlState,
    CharacterizationState,
    LedgerHeadroom,
    OauthRunway,
    EnvironmentBindings,
    PriceRankingSnapshot,
    StorageHeadroom,
    PresentImageNoPullControl,
    LegacyInventory,
    SupervisorRecovery,
    ActionDirectories,
}

const ORDERED_CHECKS: [PreflightCheckV1; 18] = [
    PreflightCheckV1::OwnerAndArchitecture,
    PreflightCheckV1::EffectAuthorityAndPolicy,
    PreflightCheckV1::CandidateBinary,
    PreflightCheckV1::GeneratedConfig,
    PreflightCheckV1::ProductionManifest,
    PreflightCheckV1::FloatingRecipe,
    PreflightCheckV1::ScheduledRegistry,
    PreflightCheckV1::ControlState,
    PreflightCheckV1::CharacterizationState,
    PreflightCheckV1::LedgerHeadroom,
    PreflightCheckV1::OauthRunway,
    PreflightCheckV1::EnvironmentBindings,
    PreflightCheckV1::PriceRankingSnapshot,
    PreflightCheckV1::StorageHeadroom,
    PreflightCheckV1::PresentImageNoPullControl,
    PreflightCheckV1::LegacyInventory,
    PreflightCheckV1::SupervisorRecovery,
    PreflightCheckV1::ActionDirectories,
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalPreflightProofV1 {
    pub(super) check: PreflightCheckV1,
    pub(super) evidence_sha256: String,
    pub(super) observed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalPreflightRefusalV1 {
    pub(super) code: String,
    pub(super) evidence_sha256: String,
    pub(super) observed_at_ms: i64,
}

pub(super) trait ZeroEffectPreflightChecks {
    fn revalidate(
        &mut self,
        check: PreflightCheckV1,
    ) -> Result<LocalPreflightProofV1, LocalPreflightRefusalV1>;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PreflightPassV1 {
    pub(super) schema_version: u16,
    pub(super) fence: PreflightFenceV1,
    pub(super) proofs: Vec<LocalPreflightProofV1>,
    pub(super) completed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PreflightRefusalV1 {
    pub(super) schema_version: u16,
    pub(super) fence: PreflightFenceV1,
    pub(super) failed_check: PreflightCheckV1,
    pub(super) code: String,
    pub(super) evidence_sha256: String,
    pub(super) observed_at_ms: i64,
    pub(super) provider_calls: u64,
    pub(super) model_calls: u64,
    pub(super) registry_effect_calls: u64,
    pub(super) runtime_effect_calls: u64,
}

fn valid_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub(super) fn run_zero_effect_preflight<C: ZeroEffectPreflightChecks>(
    fence: PreflightFenceV1,
    checks: &mut C,
) -> Result<PreflightPassV1, PreflightRefusalV1> {
    let mut proofs = Vec::with_capacity(ORDERED_CHECKS.len());
    for expected in ORDERED_CHECKS {
        match checks.revalidate(expected) {
            Ok(proof)
                if proof.check == expected
                    && local_file::valid_sha256(&proof.evidence_sha256)
                    && proof.observed_at_ms > 0 =>
            {
                proofs.push(proof)
            }
            Ok(proof) => {
                return Err(PreflightRefusalV1 {
                    schema_version: 1,
                    fence,
                    failed_check: expected,
                    code: "malformed_local_proof".into(),
                    evidence_sha256: if local_file::valid_sha256(&proof.evidence_sha256) {
                        proof.evidence_sha256
                    } else {
                        local_file::sha256_hex(b"malformed-local-preflight-proof")
                    },
                    observed_at_ms: proof.observed_at_ms.max(1),
                    provider_calls: 0,
                    model_calls: 0,
                    registry_effect_calls: 0,
                    runtime_effect_calls: 0,
                })
            }
            Err(refusal) => {
                let well_formed = valid_code(&refusal.code)
                    && local_file::valid_sha256(&refusal.evidence_sha256)
                    && refusal.observed_at_ms > 0;
                return Err(PreflightRefusalV1 {
                    schema_version: 1,
                    fence,
                    failed_check: expected,
                    code: if well_formed {
                        refusal.code
                    } else {
                        "malformed_local_refusal".into()
                    },
                    evidence_sha256: if local_file::valid_sha256(&refusal.evidence_sha256) {
                        refusal.evidence_sha256
                    } else {
                        local_file::sha256_hex(b"malformed-local-preflight-refusal")
                    },
                    observed_at_ms: refusal.observed_at_ms.max(1),
                    provider_calls: 0,
                    model_calls: 0,
                    registry_effect_calls: 0,
                    runtime_effect_calls: 0,
                });
            }
        }
    }
    let completed_at_ms = proofs
        .iter()
        .map(|proof| proof.observed_at_ms)
        .max()
        .unwrap_or(1);
    Ok(PreflightPassV1 {
        schema_version: 1,
        fence,
        proofs,
        completed_at_ms,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PlannedDirectoryBindingV1 {
    pub(super) requested_path: String,
    pub(super) canonical_path: String,
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) object_sha256: String,
}

pub(super) struct PinnedActionDirectoriesV1 {
    pub(super) trusted_root: local_file::PinnedDirectory,
    pub(super) requested_cwd: local_file::PinnedDirectory,
}

fn binding_from_snapshot(
    requested_path: &Path,
    snapshot: &local_file::DirectorySnapshot,
) -> Result<PlannedDirectoryBindingV1, BoxError> {
    let requested_path = requested_path
        .to_str()
        .ok_or("schedule preflight: directory path is not UTF-8")?;
    let canonical_path = snapshot.canonical_cwd.as_str();
    if requested_path != canonical_path {
        return Err(
            "schedule preflight: directory authority must not traverse a symlink or alias".into(),
        );
    }
    Ok(PlannedDirectoryBindingV1 {
        requested_path: requested_path.into(),
        canonical_path: canonical_path.into(),
        device: snapshot.identity.device,
        inode: snapshot.identity.inode,
        object_sha256: snapshot.identity.object_sha256.clone(),
    })
}

pub(super) fn plan_directory_binding(path: &Path) -> Result<PlannedDirectoryBindingV1, BoxError> {
    if !path.is_absolute() {
        return Err("schedule preflight: planned directory must be absolute".into());
    }
    let snapshot = local_file::snapshot_directory(path, "schedule planned directory")?;
    binding_from_snapshot(path, &snapshot)
}

fn binding_matches(
    expected: &PlannedDirectoryBindingV1,
    observed: &local_file::DirectorySnapshot,
) -> bool {
    observed.canonical_cwd.as_str() == expected.canonical_path
        && observed.identity.device == expected.device
        && observed.identity.inode == expected.inode
        && observed.identity.object_sha256 == expected.object_sha256
}

fn verify_directory_owner_mode(
    directory: &local_file::PinnedDirectory,
    expected_uid: u32,
    label: &str,
) -> Result<(), BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = directory.file_handle().metadata()?;
    if !metadata.is_dir() || metadata.uid() != expected_uid || metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "schedule preflight: {label} must be owner-owned and not group/world writable"
        )
        .into());
    }
    Ok(())
}

fn same_directory(
    left: &local_file::PinnedDirectory,
    right: &local_file::PinnedDirectory,
) -> Result<bool, BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    let left = left.file_handle().metadata()?;
    let right = right.file_handle().metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

fn pin_action_directories_with_hook<F>(
    trusted_root: &PlannedDirectoryBindingV1,
    requested_cwd: &PlannedDirectoryBindingV1,
    expected_uid: u32,
    after_snapshot: F,
) -> Result<PinnedActionDirectoriesV1, BoxError>
where
    F: FnOnce(),
{
    if trusted_root.requested_path != trusted_root.canonical_path
        || requested_cwd.requested_path != requested_cwd.canonical_path
    {
        return Err("schedule preflight: planned directory binding contains an alias".into());
    }
    let root_path = Path::new(&trusted_root.requested_path);
    let cwd_path = Path::new(&requested_cwd.requested_path);
    let root_snapshot = local_file::snapshot_directory(root_path, "action trusted root")?;
    let cwd_snapshot = local_file::snapshot_directory(cwd_path, "action requested cwd")?;
    if !binding_matches(trusted_root, &root_snapshot)
        || !binding_matches(requested_cwd, &cwd_snapshot)
    {
        return Err("schedule preflight: action-time directory identity drifted".into());
    }
    after_snapshot();
    let root = local_file::PinnedDirectory::open(
        root_path,
        &root_snapshot.canonical_cwd,
        &root_snapshot.identity,
        "action trusted root",
    )?;
    let cwd = local_file::PinnedDirectory::open(
        cwd_path,
        &cwd_snapshot.canonical_cwd,
        &cwd_snapshot.identity,
        "action requested cwd",
    )?;
    verify_directory_owner_mode(&root, expected_uid, "trusted root")?;
    verify_directory_owner_mode(&cwd, expected_uid, "requested cwd")?;

    let relative = cwd
        .canonical_path()
        .strip_prefix(root.canonical_path())
        .map_err(|_| "schedule preflight: requested cwd is outside the trusted root")?
        .to_path_buf();
    let mut traversed = root.clone();
    let mut containment_chain = vec![root.clone()];
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("schedule preflight: requested cwd containment is not canonical".into());
        };
        traversed = traversed.open_child_directory(name, "action cwd containment")?;
        verify_directory_owner_mode(&traversed, expected_uid, "cwd containment component")?;
        containment_chain.push(traversed.clone());
    }
    if !same_directory(&traversed, &cwd)?
        || !cwd.current_path_matches()
        || containment_chain.iter().any(|directory| {
            !directory.current_path_matches()
                || verify_directory_owner_mode(
                    directory,
                    expected_uid,
                    "retained containment component",
                )
                .is_err()
        })
    {
        return Err(
            "schedule preflight: requested cwd is not beneath the same retained root object".into(),
        );
    }
    Ok(PinnedActionDirectoriesV1 {
        trusted_root: root,
        requested_cwd: cwd,
    })
}

pub(super) fn pin_action_directories(
    trusted_root: &PlannedDirectoryBindingV1,
    requested_cwd: &PlannedDirectoryBindingV1,
) -> Result<PinnedActionDirectoriesV1, BoxError> {
    pin_action_directories_with_hook(
        trusted_root,
        requested_cwd,
        unsafe { libc::geteuid() },
        || {},
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OptionalExecutableSha256V1 {
    Unobserved,
    Sha256 { value: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessExecutableIdentityV1 {
    pub(super) canonical_path: String,
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) sha256: OptionalExecutableSha256V1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyProcessObservationV1 {
    pub(super) process: ProcessIdentityV1,
    pub(super) executable: ProcessExecutableIdentityV1,
    pub(super) argv: Vec<String>,
    pub(super) argv_complete: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AllowedLegacyProcessRoleV1 {
    CurrentScheduler,
    RetainedProductionServe,
    AllowedNonCompatibility,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyProcessAllowanceV1 {
    pub(super) role: AllowedLegacyProcessRoleV1,
    pub(super) required_live: bool,
    pub(super) exact: LegacyProcessObservationV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyInventoryPlanV1 {
    pub(super) schema_version: u16,
    pub(super) scheduler_candidate_sha256: String,
    pub(super) observed_at_ms: i64,
    pub(super) quiescence_at_ms: i64,
    pub(super) known_bridge_executables: Vec<ProcessExecutableIdentityV1>,
    pub(super) allowances: Vec<LegacyProcessAllowanceV1>,
    pub(super) required_ledger_import_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SealedLegacyInventoryV1 {
    pub(super) plan: LegacyInventoryPlanV1,
    pub(super) inventory_sha256: String,
}

fn preflight_hash<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| format!("schedule preflight: cannot canonicalize {label}: {error}"))?;
    let mut bytes = format!("a2a-bridge:r3d2:preflight:{label}:v1\0").into_bytes();
    bytes.extend_from_slice(&canonical);
    Ok(local_file::sha256_hex(&bytes))
}

fn executable_digest(value: &ProcessExecutableIdentityV1) -> Option<&str> {
    match &value.sha256 {
        OptionalExecutableSha256V1::Unobserved => None,
        OptionalExecutableSha256V1::Sha256 { value } => Some(value),
    }
}

fn valid_executable(value: &ProcessExecutableIdentityV1, digest_required: bool) -> bool {
    Path::new(&value.canonical_path).is_absolute()
        && value.device > 0
        && value.inode > 0
        && match executable_digest(value) {
            Some(digest) => local_file::valid_sha256(digest),
            None => !digest_required,
        }
}

fn executable_matches(
    expected: &ProcessExecutableIdentityV1,
    observed: &ProcessExecutableIdentityV1,
) -> bool {
    expected.canonical_path == observed.canonical_path
        && expected.device == observed.device
        && expected.inode == observed.inode
        && executable_digest(expected).is_some()
        && executable_digest(expected) == executable_digest(observed)
}

fn command_name(value: &LegacyProcessObservationV1) -> Option<&str> {
    value.argv.get(1).map(String::as_str)
}

fn is_serve_command(value: &LegacyProcessObservationV1) -> bool {
    value.argv_complete
        && (matches!(command_name(value), None | Some("serve"))
            || value
                .argv
                .get(1)
                .is_some_and(|argument| argument.starts_with("--")))
}

fn is_scheduler_command(value: &LegacyProcessObservationV1) -> bool {
    value.argv_complete && command_name(value) == Some("schedule-tick")
}

fn is_compatibility_command(value: &LegacyProcessObservationV1) -> bool {
    value.argv_complete && command_name(value) == Some("compatibility")
}

fn basename_is_bridge(value: &ProcessExecutableIdentityV1) -> bool {
    Path::new(&value.canonical_path)
        .file_name()
        .is_some_and(|name| name == "a2a-bridge")
}

fn validate_observation(value: &LegacyProcessObservationV1, exact: bool) -> Result<(), BoxError> {
    if value.process.pid <= 0
        || value.process.parent_pid < 0
        || value.process.process_group <= 0
        || value.process.session_id <= 0
        || !valid_executable(&value.executable, exact)
        || value.argv.len() > 256
        || value
            .argv
            .iter()
            .any(|argument| argument.len() > 64 * 1024 || argument.contains('\0'))
        || (exact && (!value.argv_complete || value.argv.is_empty()))
    {
        return Err("schedule preflight: legacy process observation is malformed".into());
    }
    Ok(())
}

fn stable_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(
            value.as_bytes().first(),
            Some(b'a'..=b'z') | Some(b'0'..=b'9')
        )
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn validate_legacy_plan(value: &LegacyInventoryPlanV1) -> Result<(), BoxError> {
    if value.schema_version != 1
        || !local_file::valid_sha256(&value.scheduler_candidate_sha256)
        || value.observed_at_ms <= 0
        || value.quiescence_at_ms < value.observed_at_ms
        || value.known_bridge_executables.is_empty()
        || value.known_bridge_executables.len() > 64
        || value.allowances.is_empty()
        || value.allowances.len() > 1_024
    {
        return Err("schedule preflight: legacy inventory plan is malformed".into());
    }
    let mut executables = BTreeSet::new();
    for executable in &value.known_bridge_executables {
        if !valid_executable(executable, true)
            || !executables.insert((
                executable.canonical_path.as_str(),
                executable.device,
                executable.inode,
                executable_digest(executable).unwrap_or_default(),
            ))
        {
            return Err(
                "schedule preflight: known bridge executable is invalid or repeated".into(),
            );
        }
    }
    let mut pids = BTreeSet::new();
    let mut schedulers = 0;
    let mut serves = 0;
    for allowance in &value.allowances {
        validate_observation(&allowance.exact, true)?;
        if !pids.insert(allowance.exact.process.pid)
            || !value
                .known_bridge_executables
                .iter()
                .any(|known| executable_matches(known, &allowance.exact.executable))
                && allowance.role != AllowedLegacyProcessRoleV1::AllowedNonCompatibility
        {
            return Err("schedule preflight: process allowance is duplicated or unbound".into());
        }
        match allowance.role {
            AllowedLegacyProcessRoleV1::CurrentScheduler => {
                schedulers += 1;
                if !allowance.required_live
                    || !is_scheduler_command(&allowance.exact)
                    || executable_digest(&allowance.exact.executable)
                        != Some(value.scheduler_candidate_sha256.as_str())
                {
                    return Err("schedule preflight: current scheduler allowance is invalid".into());
                }
            }
            AllowedLegacyProcessRoleV1::RetainedProductionServe => {
                serves += 1;
                if !allowance.required_live || !is_serve_command(&allowance.exact) {
                    return Err("schedule preflight: production serve allowance is invalid".into());
                }
            }
            AllowedLegacyProcessRoleV1::AllowedNonCompatibility => {
                if is_compatibility_command(&allowance.exact) {
                    return Err(
                        "schedule preflight: compatibility process cannot be allowed".into(),
                    );
                }
            }
        }
    }
    if schedulers != 1 || serves > 1 {
        return Err(
            "schedule preflight: inventory requires one scheduler and at most one serve".into(),
        );
    }
    let imports = value
        .required_ledger_import_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if imports.len() != value.required_ledger_import_ids.len()
        || imports.iter().any(|value| !stable_id(value))
    {
        return Err("schedule preflight: legacy ledger import ids are invalid".into());
    }
    Ok(())
}

pub(super) fn seal_legacy_inventory(
    plan: LegacyInventoryPlanV1,
) -> Result<SealedLegacyInventoryV1, BoxError> {
    validate_legacy_plan(&plan)?;
    let inventory_sha256 = preflight_hash("legacy-inventory", &plan)?;
    Ok(SealedLegacyInventoryV1 {
        plan,
        inventory_sha256,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacySafetyHoldReasonV1 {
    InventoryBindingMismatch,
    RequiredProcessMissing,
    AllowedProcessIdentityDrift,
    LegacyCompatibilityLive,
    DivergentBridgeExecutable,
    UnexpectedBridgeProcess,
    AmbiguousProviderCapableChild,
    UnreconciledLegacyAggregate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LegacyFenceOutcomeV1 {
    Clear { inventory_sha256: String },
    SafetyHold(LegacySafetyHoldReasonV1),
}

fn observation_exact(
    expected: &LegacyProcessObservationV1,
    observed: &LegacyProcessObservationV1,
) -> bool {
    expected.process == observed.process
        && executable_matches(&expected.executable, &observed.executable)
        && expected.argv == observed.argv
        && expected.argv_complete
        && observed.argv_complete
}

pub(super) fn evaluate_legacy_inventory(
    sealed: &SealedLegacyInventoryV1,
    authority_inventory_sha256: &str,
    observed: &[LegacyProcessObservationV1],
    ledger_import_ids: &BTreeSet<String>,
) -> Result<LegacyFenceOutcomeV1, BoxError> {
    validate_legacy_plan(&sealed.plan)?;
    if preflight_hash("legacy-inventory", &sealed.plan)? != sealed.inventory_sha256
        || sealed.inventory_sha256 != authority_inventory_sha256
    {
        return Ok(LegacyFenceOutcomeV1::SafetyHold(
            LegacySafetyHoldReasonV1::InventoryBindingMismatch,
        ));
    }
    let mut by_pid = BTreeMap::new();
    for value in observed {
        validate_observation(value, false)?;
        if by_pid.insert(value.process.pid, value).is_some() {
            return Err("schedule preflight: live process inventory repeats a pid".into());
        }
    }
    let allowances = sealed
        .plan
        .allowances
        .iter()
        .map(|allowance| (allowance.exact.process.pid, allowance))
        .collect::<BTreeMap<_, _>>();
    for allowance in &sealed.plan.allowances {
        match by_pid.get(&allowance.exact.process.pid) {
            Some(observed) if observation_exact(&allowance.exact, observed) => {}
            Some(_) => {
                return Ok(LegacyFenceOutcomeV1::SafetyHold(
                    LegacySafetyHoldReasonV1::AllowedProcessIdentityDrift,
                ))
            }
            None if allowance.required_live => {
                return Ok(LegacyFenceOutcomeV1::SafetyHold(
                    LegacySafetyHoldReasonV1::RequiredProcessMissing,
                ))
            }
            None => {}
        }
    }

    let bridge_pids = observed
        .iter()
        .filter(|value| {
            basename_is_bridge(&value.executable)
                || sealed
                    .plan
                    .known_bridge_executables
                    .iter()
                    .any(|known| executable_matches(known, &value.executable))
        })
        .map(|value| value.process.pid)
        .collect::<BTreeSet<_>>();
    let is_bridge_descendant = |value: &LegacyProcessObservationV1| {
        let mut parent = value.process.parent_pid;
        let mut visited = BTreeSet::new();
        while parent > 0 && visited.insert(parent) {
            if bridge_pids.contains(&parent) {
                return true;
            }
            let Some(observation) = by_pid.get(&parent) else {
                break;
            };
            parent = observation.process.parent_pid;
        }
        false
    };
    for value in observed {
        if allowances
            .get(&value.process.pid)
            .is_some_and(|allowance| observation_exact(&allowance.exact, value))
        {
            continue;
        }
        if bridge_pids.contains(&value.process.pid) {
            let reason = if is_compatibility_command(value) {
                LegacySafetyHoldReasonV1::LegacyCompatibilityLive
            } else if basename_is_bridge(&value.executable)
                && !sealed
                    .plan
                    .known_bridge_executables
                    .iter()
                    .any(|known| executable_matches(known, &value.executable))
            {
                LegacySafetyHoldReasonV1::DivergentBridgeExecutable
            } else {
                LegacySafetyHoldReasonV1::UnexpectedBridgeProcess
            };
            return Ok(LegacyFenceOutcomeV1::SafetyHold(reason));
        }
        if is_bridge_descendant(value) {
            return Ok(LegacyFenceOutcomeV1::SafetyHold(
                LegacySafetyHoldReasonV1::AmbiguousProviderCapableChild,
            ));
        }
    }
    if sealed
        .plan
        .required_ledger_import_ids
        .iter()
        .any(|id| !ledger_import_ids.contains(id))
    {
        return Ok(LegacyFenceOutcomeV1::SafetyHold(
            LegacySafetyHoldReasonV1::UnreconciledLegacyAggregate,
        ));
    }
    Ok(LegacyFenceOutcomeV1::Clear {
        inventory_sha256: sealed.inventory_sha256.clone(),
    })
}

#[cfg(target_os = "linux")]
fn host_process_pids() -> Result<Vec<i32>, BoxError> {
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|pid| *pid > 0)
        {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

#[cfg(target_os = "macos")]
fn host_process_pids() -> Result<Vec<i32>, BoxError> {
    // SAFETY: a null/zero first call asks libproc for the required process count.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 || count > 1_000_000 {
        return Err(format!(
            "schedule preflight: cannot bound host process inventory: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    let capacity = usize::try_from(count)?.saturating_add(256);
    let mut values = vec![0 as libc::pid_t; capacity];
    let bytes = i32::try_from(values.len() * std::mem::size_of::<libc::pid_t>())?;
    // SAFETY: values is live writable storage of the declared byte length.
    let returned = unsafe { libc::proc_listallpids(values.as_mut_ptr().cast(), bytes) };
    if returned < 0 || usize::try_from(returned)? > values.len() {
        return Err("schedule preflight: host process inventory changed beyond its bound".into());
    }
    values.truncate(usize::try_from(returned)?);
    values.retain(|pid| *pid > 0);
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_process_pids() -> Result<Vec<i32>, BoxError> {
    Err("schedule preflight: exact host process inventory is unsupported".into())
}

#[cfg(target_os = "linux")]
fn host_process_executable(pid: i32) -> Result<PathBuf, BoxError> {
    Ok(std::fs::read_link(format!("/proc/{pid}/exe"))?)
}

#[cfg(target_os = "macos")]
fn host_process_executable(pid: i32) -> Result<PathBuf, BoxError> {
    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: buffer is live writable storage of the provided bounded length.
    let length = unsafe {
        libc::proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len())?,
        )
    };
    if length <= 0 || usize::try_from(length)? > buffer.len() {
        return Err(format!(
            "schedule preflight: cannot inspect process executable for pid {pid}: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    buffer.truncate(usize::try_from(length)?);
    Ok(PathBuf::from(String::from_utf8(buffer)?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_process_executable(_pid: i32) -> Result<PathBuf, BoxError> {
    Err("schedule preflight: exact process executable inspection is unsupported".into())
}

#[cfg(target_os = "linux")]
fn host_process_argv(pid: i32) -> Result<Vec<String>, BoxError> {
    use std::io::Read as _;

    let file = std::fs::File::open(format!("/proc/{pid}/cmdline"))?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 || (!bytes.is_empty() && bytes.last() != Some(&0)) {
        return Err("schedule preflight: Linux process argv is unbounded or truncated".into());
    }
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    bytes
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8(value.to_vec()).map_err(Into::into))
        .collect()
}

#[cfg(target_os = "macos")]
fn host_process_argv(pid: i32) -> Result<Vec<String>, BoxError> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut length = 0_usize;
    // SAFETY: the first call uses a null output to request the exact kernel buffer size.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } == -1
        || length < std::mem::size_of::<libc::c_int>()
        || length > 1024 * 1024
    {
        return Err(format!(
            "schedule preflight: cannot bound macOS argv for pid {pid}: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    let mut bytes = vec![0_u8; length];
    // SAFETY: bytes is live writable storage of the requested bounded length.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } == -1
    {
        return Err(format!(
            "schedule preflight: cannot read macOS argv for pid {pid}: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    bytes.truncate(length);
    let argc = i32::from_ne_bytes(
        bytes[..std::mem::size_of::<i32>()]
            .try_into()
            .map_err(|_| "schedule preflight: macOS argc is truncated")?,
    );
    if !(0..=256).contains(&argc) {
        return Err("schedule preflight: macOS argc is outside the bound".into());
    }
    let mut cursor = std::mem::size_of::<i32>();
    while cursor < bytes.len() && bytes[cursor] != 0 {
        cursor += 1;
    }
    while cursor < bytes.len() && bytes[cursor] == 0 {
        cursor += 1;
    }
    let mut argv = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor] != 0 {
            cursor += 1;
        }
        if cursor == bytes.len() {
            return Err("schedule preflight: macOS argv is truncated".into());
        }
        argv.push(String::from_utf8(bytes[start..cursor].to_vec())?);
        while cursor < bytes.len() && bytes[cursor] == 0 {
            cursor += 1;
        }
    }
    Ok(argv)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_process_argv(_pid: i32) -> Result<Vec<String>, BoxError> {
    Err("schedule preflight: exact process argv inspection is unsupported".into())
}

fn executable_identity(
    path: &Path,
    require_digest: bool,
) -> Result<ProcessExecutableIdentityV1, BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    let canonical = std::fs::canonicalize(path)?;
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err("schedule preflight: process executable is not a regular file".into());
    }
    let sha256 = if require_digest {
        OptionalExecutableSha256V1::Sha256 {
            value: local_file::read_regular_file_bounded(
                &canonical,
                "legacy process executable",
                512 * 1024 * 1024,
            )?
            .sha256,
        }
    } else {
        OptionalExecutableSha256V1::Unobserved
    };
    Ok(ProcessExecutableIdentityV1 {
        canonical_path: canonical
            .to_str()
            .ok_or("schedule preflight: executable path is not UTF-8")?
            .into(),
        device: metadata.dev(),
        inode: metadata.ino(),
        sha256,
    })
}

/// Capture exact bridge/allowed process identity and enough ancestry-only observations to detect
/// an unapproved provider-capable child. This function is read-only and never signals a process.
pub(super) fn capture_host_legacy_processes(
    sealed: &SealedLegacyInventoryV1,
) -> Result<Vec<LegacyProcessObservationV1>, BoxError> {
    validate_legacy_plan(&sealed.plan)?;
    let expected_pids = sealed
        .plan
        .allowances
        .iter()
        .map(|allowance| allowance.exact.process.pid)
        .collect::<BTreeSet<_>>();
    let mut observations = Vec::new();
    for pid in host_process_pids()? {
        let Some(first) = compatibility_process_group::process_identity(pid)? else {
            continue;
        };
        let executable_path = match host_process_executable(pid) {
            Ok(path) => path,
            Err(error) => {
                if compatibility_process_group::process_identity(pid)?.is_none() {
                    continue;
                }
                return Err(error);
            }
        };
        let basename_bridge = executable_path
            .file_name()
            .is_some_and(|name| name == "a2a-bridge");
        let preliminary = executable_identity(&executable_path, false)?;
        let known_bridge = sealed.plan.known_bridge_executables.iter().any(|known| {
            known.canonical_path == preliminary.canonical_path
                && known.device == preliminary.device
                && known.inode == preliminary.inode
        });
        let exact_required = basename_bridge || known_bridge || expected_pids.contains(&pid);
        let executable = executable_identity(&executable_path, exact_required)?;
        let (argv, argv_complete) = if exact_required {
            (host_process_argv(pid)?, true)
        } else {
            (Vec::new(), false)
        };
        match compatibility_process_group::process_identity(pid)? {
            Some(second) if second == first => observations.push(LegacyProcessObservationV1 {
                process: first,
                executable,
                argv,
                argv_complete,
            }),
            Some(_) => {
                return Err("schedule preflight: process identity drifted during inventory".into())
            }
            None => {}
        }
    }
    observations.sort_by_key(|value| value.process.pid);
    Ok(observations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_process_group::ProcessStartMarkerV1;
    use std::os::unix::fs::PermissionsExt as _;

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    #[derive(Default)]
    struct ForbiddenEffects {
        provider: u64,
        models: u64,
        registry: u64,
        runtime: u64,
    }

    struct FakeChecks {
        fail: Option<PreflightCheckV1>,
        malformed: Option<PreflightCheckV1>,
        observed: Vec<PreflightCheckV1>,
        forbidden: ForbiddenEffects,
    }

    impl FakeChecks {
        fn new(fail: Option<PreflightCheckV1>) -> Self {
            Self {
                fail,
                malformed: None,
                observed: Vec::new(),
                forbidden: ForbiddenEffects::default(),
            }
        }
    }

    impl ZeroEffectPreflightChecks for FakeChecks {
        fn revalidate(
            &mut self,
            check: PreflightCheckV1,
        ) -> Result<LocalPreflightProofV1, LocalPreflightRefusalV1> {
            self.observed.push(check);
            if self.fail == Some(check) {
                return Err(LocalPreflightRefusalV1 {
                    code: format!("{:?}_blocked", check).to_ascii_lowercase(),
                    evidence_sha256: digest('a'),
                    observed_at_ms: 10,
                });
            }
            Ok(LocalPreflightProofV1 {
                check,
                evidence_sha256: if self.malformed == Some(check) {
                    "bad".into()
                } else {
                    digest('b')
                },
                observed_at_ms: 10,
            })
        }
    }

    #[test]
    fn preflight_hash_is_one_domain_separated_canonical_payload() {
        let value = vec!["alpha", "beta"];
        let canonical = serde_json::to_vec(&value).unwrap();
        let mut expected = b"a2a-bridge:r3d2:preflight:fixture:v1\0".to_vec();
        expected.extend_from_slice(&canonical);

        assert_eq!(
            preflight_hash("fixture", &value).unwrap(),
            local_file::sha256_hex(&expected)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn host_process_primitives_capture_the_current_process_read_only() {
        let pid = std::process::id() as i32;
        assert!(host_process_pids().unwrap().contains(&pid));

        let executable = host_process_executable(pid).unwrap();
        assert!(executable.is_absolute());
        assert!(std::fs::metadata(&executable).unwrap().is_file());

        let argv = host_process_argv(pid).unwrap();
        assert!(!argv.is_empty());
        assert!(!argv[0].is_empty());

        let identity = compatibility_process_group::process_identity(pid)
            .unwrap()
            .unwrap();
        assert_eq!(identity.pid, pid);
        assert!(identity.process_group > 0);
        assert!(identity.session_id > 0);
    }

    #[test]
    fn initial_and_final_fences_run_the_same_complete_zero_effect_checklist() {
        for fence in [PreflightFenceV1::Initial, PreflightFenceV1::Final] {
            let mut checks = FakeChecks::new(None);
            let pass = run_zero_effect_preflight(fence, &mut checks).unwrap();
            assert_eq!(checks.observed, ORDERED_CHECKS);
            assert_eq!(pass.proofs.len(), ORDERED_CHECKS.len());
            assert_eq!(pass.fence, fence);
            assert_eq!(
                (
                    checks.forbidden.provider,
                    checks.forbidden.models,
                    checks.forbidden.registry,
                    checks.forbidden.runtime,
                ),
                (0, 0, 0, 0)
            );
        }
    }

    #[test]
    fn every_preflight_failure_is_typed_and_has_zero_effect_calls_at_both_fences() {
        for fence in [PreflightFenceV1::Initial, PreflightFenceV1::Final] {
            for failed in ORDERED_CHECKS {
                let mut checks = FakeChecks::new(Some(failed));
                let refusal = run_zero_effect_preflight(fence, &mut checks).unwrap_err();
                assert_eq!(refusal.failed_check, failed);
                assert_eq!(refusal.fence, fence);
                assert!(valid_code(&refusal.code));
                assert_eq!(
                    (
                        refusal.provider_calls,
                        refusal.model_calls,
                        refusal.registry_effect_calls,
                        refusal.runtime_effect_calls,
                        checks.forbidden.provider,
                        checks.forbidden.models,
                        checks.forbidden.registry,
                        checks.forbidden.runtime,
                    ),
                    (0, 0, 0, 0, 0, 0, 0, 0)
                );
                assert_eq!(checks.observed.last(), Some(&failed));
            }
        }
    }

    #[test]
    fn malformed_local_proof_fails_closed_without_skipping_to_an_effect() {
        let mut checks = FakeChecks::new(None);
        checks.malformed = Some(PreflightCheckV1::LedgerHeadroom);
        let refusal = run_zero_effect_preflight(PreflightFenceV1::Final, &mut checks).unwrap_err();
        assert_eq!(refusal.failed_check, PreflightCheckV1::LedgerHeadroom);
        assert_eq!(refusal.code, "malformed_local_proof");
        assert_eq!(
            (
                checks.forbidden.provider,
                checks.forbidden.models,
                checks.forbidden.registry,
                checks.forbidden.runtime,
            ),
            (0, 0, 0, 0)
        );
    }

    fn private_directory(path: &Path) {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn action_tree() -> (
        tempfile::TempDir,
        PathBuf,
        PathBuf,
        PlannedDirectoryBindingV1,
        PlannedDirectoryBindingV1,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(temp.path()).unwrap();
        let root = base.join("trusted");
        let cwd = root.join("repo");
        private_directory(&root);
        private_directory(&cwd);
        let root_binding = plan_directory_binding(&root).unwrap();
        let cwd_binding = plan_directory_binding(&cwd).unwrap();
        (temp, root, cwd, root_binding, cwd_binding)
    }

    #[test]
    fn action_time_directories_are_pinned_beneath_the_same_owner_root_object() {
        let (_temp, _root, _cwd, root_binding, cwd_binding) = action_tree();
        let pinned = pin_action_directories(&root_binding, &cwd_binding).unwrap();
        assert!(pinned.trusted_root.current_path_matches());
        assert!(pinned.requested_cwd.current_path_matches());
    }

    #[test]
    fn action_time_directory_outside_missing_alias_owner_and_mode_fail_closed() {
        let (_temp, _root, cwd, root_binding, cwd_binding) = action_tree();
        let outside = tempfile::tempdir().unwrap();
        let outside_path = std::fs::canonicalize(outside.path()).unwrap();
        let outside_binding = plan_directory_binding(&outside_path).unwrap();
        assert!(pin_action_directories(&root_binding, &outside_binding).is_err());

        std::fs::remove_dir(&cwd).unwrap();
        assert!(pin_action_directories(&root_binding, &cwd_binding).is_err());

        let (_temp, root, cwd, root_binding, cwd_binding) = action_tree();
        std::fs::set_permissions(&cwd, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(pin_action_directories(&root_binding, &cwd_binding).is_err());
        std::fs::set_permissions(&cwd, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(pin_action_directories_with_hook(
            &root_binding,
            &cwd_binding,
            unsafe { libc::geteuid() }.saturating_add(1),
            || {},
        )
        .is_err());

        let alias = root.join("repo-alias");
        std::os::unix::fs::symlink(&cwd, &alias).unwrap();
        assert!(plan_directory_binding(&alias).is_err());
    }

    #[test]
    fn action_time_root_or_cwd_replacement_rename_and_symlink_swap_refuse() {
        let (_temp, _root, cwd, root_binding, cwd_binding) = action_tree();
        let moved = cwd.with_file_name("repo-old");
        assert!(pin_action_directories_with_hook(
            &root_binding,
            &cwd_binding,
            unsafe { libc::geteuid() },
            || {
                std::fs::rename(&cwd, &moved).unwrap();
                private_directory(&cwd);
            },
        )
        .is_err());

        let (_temp, _root, cwd, root_binding, cwd_binding) = action_tree();
        let moved = cwd.with_file_name("repo-target");
        assert!(pin_action_directories_with_hook(
            &root_binding,
            &cwd_binding,
            unsafe { libc::geteuid() },
            || {
                std::fs::rename(&cwd, &moved).unwrap();
                std::os::unix::fs::symlink(&moved, &cwd).unwrap();
            },
        )
        .is_err());

        let (_temp, root, _cwd, root_binding, cwd_binding) = action_tree();
        let moved = root.with_file_name("trusted-old");
        assert!(pin_action_directories_with_hook(
            &root_binding,
            &cwd_binding,
            unsafe { libc::geteuid() },
            || {
                std::fs::rename(&root, &moved).unwrap();
                private_directory(&root);
            },
        )
        .is_err());
    }

    fn executable(path: &str, device: u64, inode: u64, ch: char) -> ProcessExecutableIdentityV1 {
        ProcessExecutableIdentityV1 {
            canonical_path: path.into(),
            device,
            inode,
            sha256: OptionalExecutableSha256V1::Sha256 { value: digest(ch) },
        }
    }

    fn process(
        pid: i32,
        parent_pid: i32,
        executable: ProcessExecutableIdentityV1,
        argv: &[&str],
    ) -> LegacyProcessObservationV1 {
        LegacyProcessObservationV1 {
            process: ProcessIdentityV1 {
                pid,
                parent_pid,
                process_group: pid,
                session_id: 1,
                start: ProcessStartMarkerV1::MacosEpochMicros {
                    seconds: pid as u64,
                    microseconds: 0,
                },
            },
            executable,
            argv: argv.iter().map(|value| (*value).into()).collect(),
            argv_complete: true,
        }
    }

    fn legacy_fixture() -> (
        SealedLegacyInventoryV1,
        Vec<LegacyProcessObservationV1>,
        BTreeSet<String>,
    ) {
        let current = executable("/trusted/current/a2a-bridge", 1, 10, 'a');
        let retained = executable("/trusted/retained/a2a-bridge", 1, 20, 'b');
        let scheduler = process(100, 1, current.clone(), &["a2a-bridge", "schedule-tick"]);
        let serve = process(
            200,
            1,
            retained.clone(),
            &["a2a-bridge", "serve", "--config", "/trusted/config"],
        );
        let import = "legacy-import-1".to_owned();
        let sealed = seal_legacy_inventory(LegacyInventoryPlanV1 {
            schema_version: 1,
            scheduler_candidate_sha256: digest('a'),
            observed_at_ms: 10,
            quiescence_at_ms: 20,
            known_bridge_executables: vec![current, retained],
            allowances: vec![
                LegacyProcessAllowanceV1 {
                    role: AllowedLegacyProcessRoleV1::CurrentScheduler,
                    required_live: true,
                    exact: scheduler.clone(),
                },
                LegacyProcessAllowanceV1 {
                    role: AllowedLegacyProcessRoleV1::RetainedProductionServe,
                    required_live: true,
                    exact: serve.clone(),
                },
            ],
            required_ledger_import_ids: vec![import.clone()],
        })
        .unwrap();
        (sealed, vec![scheduler, serve], BTreeSet::from([import]))
    }

    #[test]
    fn exact_production_serve_is_allowed_at_both_legacy_fences() {
        let (sealed, observed, imports) = legacy_fixture();
        for _fence in [PreflightFenceV1::Initial, PreflightFenceV1::Final] {
            assert_eq!(
                evaluate_legacy_inventory(&sealed, &sealed.inventory_sha256, &observed, &imports,)
                    .unwrap(),
                LegacyFenceOutcomeV1::Clear {
                    inventory_sha256: sealed.inventory_sha256.clone(),
                }
            );
        }
    }

    #[test]
    fn legacy_compatibility_ambiguous_child_and_divergent_binary_hold() {
        let (sealed, observed, imports) = legacy_fixture();
        let legacy_executable = sealed.plan.known_bridge_executables[1].clone();

        let mut compatibility = observed.clone();
        compatibility.push(process(
            300,
            1,
            legacy_executable,
            &["a2a-bridge", "compatibility", "run"],
        ));
        assert_eq!(
            evaluate_legacy_inventory(&sealed, &sealed.inventory_sha256, &compatibility, &imports,)
                .unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(LegacySafetyHoldReasonV1::LegacyCompatibilityLive)
        );

        let mut child = observed.clone();
        child.push(process(
            301,
            200,
            executable("/usr/local/bin/node", 1, 30, 'c'),
            &["node", "adapter.js"],
        ));
        assert_eq!(
            evaluate_legacy_inventory(&sealed, &sealed.inventory_sha256, &child, &imports,)
                .unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(
                LegacySafetyHoldReasonV1::AmbiguousProviderCapableChild
            )
        );

        let mut divergent = observed;
        divergent.push(process(
            302,
            1,
            executable("/untrusted/a2a-bridge", 1, 40, 'd'),
            &["a2a-bridge", "serve"],
        ));
        assert_eq!(
            evaluate_legacy_inventory(&sealed, &sealed.inventory_sha256, &divergent, &imports,)
                .unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(LegacySafetyHoldReasonV1::DivergentBridgeExecutable)
        );
    }

    #[test]
    fn missing_drifted_or_unreconciled_legacy_state_holds_without_process_action() {
        let (sealed, mut observed, imports) = legacy_fixture();
        observed.retain(|value| value.process.pid != 200);
        assert_eq!(
            evaluate_legacy_inventory(&sealed, &sealed.inventory_sha256, &observed, &imports,)
                .unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(LegacySafetyHoldReasonV1::RequiredProcessMissing)
        );

        let (sealed, mut observed, imports) = legacy_fixture();
        observed[1].argv.push("--changed".into());
        assert_eq!(
            evaluate_legacy_inventory(&sealed, &sealed.inventory_sha256, &observed, &imports,)
                .unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(LegacySafetyHoldReasonV1::AllowedProcessIdentityDrift)
        );

        let (sealed, mut observed, imports) = legacy_fixture();
        observed[1].process.start = ProcessStartMarkerV1::MacosEpochMicros {
            seconds: 999,
            microseconds: 0,
        };
        assert_eq!(
            evaluate_legacy_inventory(&sealed, &sealed.inventory_sha256, &observed, &imports,)
                .unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(LegacySafetyHoldReasonV1::AllowedProcessIdentityDrift)
        );

        let (sealed, observed, _imports) = legacy_fixture();
        assert_eq!(
            evaluate_legacy_inventory(
                &sealed,
                &sealed.inventory_sha256,
                &observed,
                &BTreeSet::new(),
            )
            .unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(LegacySafetyHoldReasonV1::UnreconciledLegacyAggregate)
        );
        assert_eq!(
            evaluate_legacy_inventory(&sealed, &digest('f'), &observed, &BTreeSet::new()).unwrap(),
            LegacyFenceOutcomeV1::SafetyHold(LegacySafetyHoldReasonV1::InventoryBindingMismatch)
        );
    }
}
