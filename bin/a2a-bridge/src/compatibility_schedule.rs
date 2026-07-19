//! R3d0 default-off scheduling policy, schema, and canonical-identity foundation.
//!
//! This module is deliberately effect-free. It parses bounded local files, validates the complete
//! checked-in characterization inventory, and derives canonical hashes. It does not read credentials,
//! access a registry or container runtime, spawn an agent, publish a check, or mutate operator state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::compatibility_schedule_schema::{
    EffectiveIdentityV1, FingerprintV1, OptionalTextV1,
    ProfileSourceKindV1 as SchemaProfileSourceKindV1, ProfileSourceRefV1,
};
use crate::{compatibility, compatibility_resolution, local_file, BoxError};

const MAX_FOUNDATION_FILE_BYTES: u64 = 1024 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_CASES: usize = 64;
const MAX_REQUIRED_ENV: usize = 32;
const MAX_DEFERRED_PROFILES: usize = 32;
const MAX_TIMEOUT_SECS: u64 = 900;
const MAX_TOKENS: u64 = 1_000_000;
const MAX_COST_MICROUSD: u64 = 100_000_000;
const OWNER_APPROVED_TRUSTED_CWD_ROOT: &str = "/Users/wesleyjinks/code";
pub(super) const EXPECTED_SUPPORT_PROFILES: [&str; 4] = [
    "claude-host-acp-044-fable",
    "claude-reader-055-fable",
    "codex-host-bridge-gpt56-sol",
    "codex-reader-bridge-gpt56-sol",
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum TriggerKindV1 {
    ManualCharacterization,
    ManualCompatibility,
    Daily,
    ScheduledMain,
    TestMerge,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum EffectClassV1 {
    ProviderPrompt,
    RegistryRead,
    ImageInspect,
    ImageBuild,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReplicationModeV1 {
    OwnerIcloud,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EffectCapsV1 {
    pub(super) timeout_secs: u64,
    pub(super) max_tokens: u64,
    pub(super) max_cost_microusd: u64,
    pub(super) attempts: u8,
    pub(super) retry_cap: u8,
    pub(super) fallback_cap: u8,
}

impl EffectCapsV1 {
    pub(super) fn validate(&self, label: &str) -> Result<(), BoxError> {
        if self.timeout_secs == 0 || self.timeout_secs > MAX_TIMEOUT_SECS {
            return Err(format!(
                "schedule foundation: {label}.timeout_secs must be in 1..={MAX_TIMEOUT_SECS}"
            )
            .into());
        }
        if self.max_tokens == 0 || self.max_tokens > MAX_TOKENS {
            return Err(format!(
                "schedule foundation: {label}.max_tokens must be in 1..={MAX_TOKENS}"
            )
            .into());
        }
        if self.max_cost_microusd > MAX_COST_MICROUSD {
            return Err(format!(
                "schedule foundation: {label}.max_cost_microusd exceeds {MAX_COST_MICROUSD}"
            )
            .into());
        }
        if self.attempts != 1 || self.retry_cap != 0 || self.fallback_cap != 0 {
            return Err(format!(
                "schedule foundation: {label} must allow exactly one attempt with retry/fallback zero"
            )
            .into());
        }
        Ok(())
    }

    pub(super) fn within(&self, maximum: &Self, label: &str) -> Result<(), BoxError> {
        if self.timeout_secs > maximum.timeout_secs
            || self.max_tokens > maximum.max_tokens
            || self.max_cost_microusd > maximum.max_cost_microusd
            || self.attempts > maximum.attempts
            || self.retry_cap > maximum.retry_cap
            || self.fallback_cap > maximum.fallback_cap
        {
            return Err(format!(
                "schedule foundation: {label} exceeds the checked-in profile maximum"
            )
            .into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StoragePolicyV1 {
    hot_root: String,
    hot_cap_bytes: u64,
    hot_index_cap_bytes: u64,
    hot_scratch_cap_bytes: u64,
    hot_sealed_cap_bytes: u64,
    cold_root: String,
    cold_cap_bytes: u64,
    replication_mode: ReplicationModeV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SchedulePolicyV1 {
    schema_version: u16,
    environment_owner: String,
    trusted_cwd_root: PathBuf,
    repository: String,
    fixed_prompt_contract: String,
    artifact_template: String,
    price_ranking_authority: String,
    price_snapshot_max_age_secs: u64,
    scheduled_registry: PathBuf,
    characterization_inventory: PathBuf,
    production_manifest: PathBuf,
    floating_recipes: PathBuf,
    allowed_triggers: Vec<TriggerKindV1>,
    allowed_effects: Vec<EffectClassV1>,
    storage: StoragePolicyV1,
    profile_maxima: EffectCapsV1,
    #[serde(default)]
    deferred_profiles: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CharacterizationStateV1 {
    CharacterizationRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ScheduledLaneV1 {
    FloatingCurrent,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduledClassificationV1 {
    Canary,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
enum ExpectedStatusV1 {
    Pass,
    Fail,
    Unknown,
    Stale,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum EvidencePurposeV1 {
    ProviderPathAdvisory,
    ClaimedSupportGate,
    Characterization,
    ManualDiagnostic,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduleExecutionModeV1 {
    Host,
    ContainerRo,
    RemoteApi,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduleAuthPathV1 {
    ApiKeyEnv,
    PreAuthenticated,
    Automatic,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduleRedactionV1 {
    Strict,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct RequiredEnvironmentV1 {
    name: String,
    #[serde(default)]
    one_of: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ScheduleArtifactPolicyV1 {
    retention_days: u16,
    redaction: ScheduleRedactionV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ScheduledCaseV1 {
    id: String,
    state: CharacterizationStateV1,
    provider_family: String,
    capability: String,
    lane: ScheduledLaneV1,
    classification: ScheduledClassificationV1,
    evidence_purpose: EvidencePurposeV1,
    evidence_path: String,
    probe: String,
    expected_status: ExpectedStatusV1,
    execution_mode: ScheduleExecutionModeV1,
    os: String,
    architecture: String,
    environment_owner: String,
    config: PathBuf,
    agent: String,
    model: String,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    expected_effective_model: String,
    #[serde(default)]
    expected_effective_effort: Option<String>,
    #[serde(default)]
    expected_effective_mode: Option<String>,
    session_cwd: PathBuf,
    auth_path: ScheduleAuthPathV1,
    #[serde(default)]
    credential_env: Option<String>,
    #[serde(default)]
    required_env: Vec<RequiredEnvironmentV1>,
    #[serde(default)]
    resolution_case: Option<String>,
    config_template: String,
    adapter_family: String,
    agent_cli_family: String,
    #[serde(default)]
    image_family: Option<String>,
    allowed_effects: Vec<EffectClassV1>,
    caps: EffectCapsV1,
    artifact: ScheduleArtifactPolicyV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ScheduledCaseRegistryV1 {
    schema_version: u16,
    #[serde(default)]
    cases: Vec<ScheduledCaseV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum ProfileSourceKindV1 {
    ScheduledAdvisory,
    ClaimedSupportGate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CharacterizationProfileReferenceV1 {
    id: String,
    source_kind: ProfileSourceKindV1,
    source_id: String,
    profile_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CharacterizationProfileInventoryV1 {
    schema_version: u16,
    #[serde(default)]
    profiles: Vec<CharacterizationProfileReferenceV1>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanonicalProfileInputV1 {
    schema_version: u16,
    source_kind: ProfileSourceKindV1,
    source_id: String,
    repository: String,
    source_schema_version: u16,
    lane: String,
    classification: String,
    evidence_purpose: EvidencePurposeV1,
    evidence_path: String,
    probe: String,
    expected_status: ExpectedStatusV1,
    execution_mode: String,
    provider_family: String,
    agent: String,
    capability: String,
    adapter_family: String,
    agent_cli_family: String,
    image_family: Option<String>,
    auth_path: String,
    credential_env_name: Option<String>,
    required_env: Vec<RequiredEnvironmentV1>,
    environment_owner: String,
    os: String,
    architecture: String,
    session_cwd: String,
    requested_model: String,
    requested_effort: Option<String>,
    requested_mode: Option<String>,
    expected_effective_model: String,
    expected_effective_effort: Option<String>,
    expected_effective_mode: Option<String>,
    config_template: String,
    config_template_sha256: String,
    #[serde(skip_serializing)]
    exact_config_sha256: String,
    resolution_constraint_sha256: Option<String>,
    allowed_effects: Vec<EffectClassV1>,
    fixed_prompt_contract: String,
    artifact_template: String,
    artifact_retention_days: u16,
    artifact_redaction: String,
    maximum_caps: EffectCapsV1,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanonicalSandboxTemplateV1 {
    image_family: String,
    mount: String,
    access: String,
    egress: String,
    network: String,
    proxy: String,
    volumes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanonicalConfigTemplateV1 {
    schema_version: u16,
    template_id: String,
    default_agent: String,
    agent_id: String,
    kind: String,
    command_family: Option<String>,
    model: String,
    effort: Option<String>,
    mode: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    pre_authenticated: bool,
    args: Vec<String>,
    allowed_cwd_root: Option<String>,
    sandbox: Option<CanonicalSandboxTemplateV1>,
    allowed_commands: Vec<String>,
    server_addr: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProfilePolicyBundleInputV1 {
    schema_version: u16,
    policy_sha256: String,
    registry_sha256: String,
    inventory_sha256: String,
    floating_recipes_sha256: String,
    scheduled_profiles: BTreeMap<String, String>,
    claimed_support_profiles: BTreeMap<String, String>,
    config_templates: BTreeMap<String, String>,
    allowed_effects: Vec<EffectClassV1>,
    profile_maxima: EffectCapsV1,
    fixed_prompt_contract: String,
    artifact_template: String,
}

#[derive(Debug)]
struct LoadedToml<T> {
    value: T,
    canonical_path: PathBuf,
    sha256: String,
    file_identity: local_file::RegularFileIdentity,
}

#[derive(Debug)]
pub(super) struct LoadedScheduleFoundation {
    pub(super) scheduled_profile_count: usize,
    pub(super) claimed_support_profile_count: usize,
    pub(super) profile_policy_bundle_sha256: String,
    pub(super) scheduled_profiles: BTreeMap<String, FoundationProfileBindingV1>,
    pub(super) claimed_support_profiles: BTreeMap<String, FoundationProfileBindingV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FoundationProfileBindingV1 {
    pub(super) source: ProfileSourceRefV1,
    pub(super) characterization_profile: FingerprintV1,
    pub(super) provider_family: String,
    pub(super) evidence_purpose: EvidencePurposeV1,
    pub(super) requested_identity: EffectiveIdentityV1,
    pub(super) expected_effective_identity: EffectiveIdentityV1,
    pub(super) maximum_caps: EffectCapsV1,
    pub(super) allowed_effects: Vec<EffectClassV1>,
    pub(super) config_template_sha256: String,
    pub(super) exact_config_sha256: String,
}

fn optional_identity_text(value: &Option<String>) -> OptionalTextV1 {
    match value {
        Some(value) => OptionalTextV1::Text {
            value: value.clone(),
        },
        None => OptionalTextV1::Absent,
    }
}

fn foundation_profile_binding(
    profile: &CanonicalProfileInputV1,
    source_kind: SchemaProfileSourceKindV1,
    source_sha256: String,
    row_sha256: String,
    profile_sha256: String,
) -> FoundationProfileBindingV1 {
    FoundationProfileBindingV1 {
        source: ProfileSourceRefV1 {
            kind: source_kind,
            schema_version: 1,
            source_sha256,
            row_id: profile.source_id.clone(),
            row_sha256,
        },
        characterization_profile: FingerprintV1 {
            schema_version: 1,
            sha256: profile_sha256,
        },
        provider_family: profile.provider_family.clone(),
        evidence_purpose: profile.evidence_purpose,
        requested_identity: EffectiveIdentityV1 {
            model: profile.requested_model.clone(),
            effort: optional_identity_text(&profile.requested_effort),
            mode: optional_identity_text(&profile.requested_mode),
        },
        expected_effective_identity: EffectiveIdentityV1 {
            model: profile.expected_effective_model.clone(),
            effort: optional_identity_text(&profile.expected_effective_effort),
            mode: optional_identity_text(&profile.expected_effective_mode),
        },
        maximum_caps: profile.maximum_caps.clone(),
        allowed_effects: profile.allowed_effects.clone(),
        config_template_sha256: profile.config_template_sha256.clone(),
        exact_config_sha256: profile.exact_config_sha256.clone(),
    }
}

#[derive(Clone, Debug)]
struct CapturedFoundationFile {
    canonical_path: PathBuf,
    sha256: String,
    file_identity: local_file::RegularFileIdentity,
    label: String,
    max_bytes: u64,
}

fn bounded_text(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "schedule foundation: {label} must be non-empty, unpadded, control-free UTF-8 of at most {MAX_TEXT_BYTES} bytes"
        )
        .into());
    }
    if compatibility::looks_like_secret(value) {
        return Err(format!("schedule foundation: {label} contains secret-shaped material").into());
    }
    Ok(())
}

fn stable_id(label: &str, value: &str) -> Result<(), BoxError> {
    bounded_text(label, value)?;
    if value.len() > MAX_ID_BYTES {
        return Err(format!("schedule foundation: {label} exceeds {MAX_ID_BYTES} bytes").into());
    }
    let mut bytes = value.bytes();
    if !matches!(bytes.next(), Some(b'a'..=b'z') | Some(b'0'..=b'9'))
        || !bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(format!("schedule foundation: {label} must be a lowercase stable id").into());
    }
    Ok(())
}

fn env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z') | Some(b'_'))
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && value.len() <= MAX_ID_BYTES
}

fn validate_sha256(label: &str, value: &str) -> Result<(), BoxError> {
    if !local_file::valid_sha256(value) || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(
            format!("schedule foundation: {label} must be one lowercase SHA-256 digest").into(),
        );
    }
    Ok(())
}

fn validate_relative_path(label: &str, path: &Path) -> Result<(), BoxError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "schedule foundation: {label} must be a non-empty relative path without traversal"
        )
        .into());
    }
    Ok(())
}

fn resolve_trusted_session_cwd(
    label: &str,
    path: &Path,
    trusted_root: &Path,
) -> Result<PathBuf, BoxError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
        || !path.starts_with(trusted_root)
    {
        return Err(format!(
            "schedule foundation: {label} must be an absolute traversal-free path under the owner-approved trusted cwd root"
        )
        .into());
    }

    match std::fs::symlink_metadata(trusted_root) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The checked-in foundation is also validated on non-owner CI hosts where this exact
            // owner-only root does not exist. Static lexical containment remains deterministic
            // there; an owner host with the root present must complete object resolution below.
            return Ok(path.to_path_buf());
        }
        Err(error) => {
            return Err(format!(
                "schedule foundation: cannot inspect the owner-approved trusted cwd root for {label}: {error}"
            )
            .into())
        }
    }

    let canonical_root = std::fs::canonicalize(trusted_root).map_err(|error| {
        format!(
            "schedule foundation: cannot resolve the owner-approved trusted cwd root for {label}: {error}"
        )
    })?;
    if !canonical_root.is_dir() {
        return Err(format!(
            "schedule foundation: owner-approved trusted cwd root for {label} is not a directory"
        )
        .into());
    }
    let canonical_path = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "schedule foundation: {label} is not a resolvable directory under the owner-approved trusted cwd root: {error}"
        )
    })?;
    if !canonical_path.is_dir() || !canonical_path.starts_with(&canonical_root) {
        return Err(format!(
            "schedule foundation: {label} must resolve to a directory under the owner-approved trusted cwd root"
        )
        .into());
    }
    Ok(canonical_path)
}

fn load_toml<T: DeserializeOwned>(path: &Path, label: &str) -> Result<LoadedToml<T>, BoxError> {
    let snapshot = local_file::read_regular_file_bounded(path, label, MAX_FOUNDATION_FILE_BYTES)?;
    let text = std::str::from_utf8(&snapshot.bytes)
        .map_err(|_| format!("schedule foundation: {label} must be UTF-8"))?;
    if compatibility::looks_like_secret(text) {
        return Err(format!("schedule foundation: {label} contains secret-shaped material").into());
    }
    let value = toml::from_str(text)
        .map_err(|error| format!("schedule foundation: invalid {label}: {error}"))?;
    Ok(LoadedToml {
        value,
        canonical_path: snapshot.canonical_path,
        sha256: snapshot.sha256,
        file_identity: snapshot.identity,
    })
}

fn ensure_snapshot_within_root(
    root: &Path,
    canonical_path: &Path,
    label: &str,
) -> Result<(), BoxError> {
    if !canonical_path.starts_with(root) {
        return Err(format!("schedule foundation: {label} escapes the foundation root").into());
    }
    Ok(())
}

fn load_foundation_toml<T: DeserializeOwned>(
    root: &Path,
    path: &Path,
    label: &str,
) -> Result<LoadedToml<T>, BoxError> {
    let loaded = load_toml(path, label)?;
    ensure_snapshot_within_root(root, &loaded.canonical_path, label)?;
    Ok(loaded)
}

fn canonical_hash<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| format!("schedule foundation: cannot canonicalize {label}: {error}"))?;
    let mut domain_separated = format!("a2a-bridge:r3d0:{label}:v1\0").into_bytes();
    domain_separated.extend_from_slice(&canonical);
    Ok(local_file::sha256_hex(&domain_separated))
}

fn exact_toml_keys(
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
    label: &str,
) -> Result<(), BoxError> {
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    let unknown = table
        .keys()
        .filter(|key| !allowed.contains(key.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(
            format!("schedule foundation: {label} contains unknown fields {unknown:?}").into(),
        );
    }
    Ok(())
}

fn optional_toml_string(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    label: &str,
) -> Result<Option<String>, BoxError> {
    table
        .get(field)
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                format!("schedule foundation: {label}.{field} must be a string").into()
            })
        })
        .transpose()
}

fn toml_string_array(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    label: &str,
) -> Result<Vec<String>, BoxError> {
    let Some(value) = table.get(field) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| format!("schedule foundation: {label}.{field} must be an array"))?;
    values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                format!("schedule foundation: {label}.{field} entries must be strings").into()
            })
        })
        .collect()
}

fn canonical_config_template_hash(
    template_id: &str,
    image_family: Option<&str>,
    value: &toml::Value,
) -> Result<(String, CanonicalConfigTemplateV1), BoxError> {
    stable_id("config template id", template_id)?;
    let root = value
        .as_table()
        .ok_or("schedule foundation: config template must be a TOML table")?;
    exact_toml_keys(
        root,
        &[
            "default",
            "allowed_cwd_root",
            "agents",
            "registry",
            "server",
        ],
        "config root",
    )?;
    let default = root
        .get("default")
        .and_then(toml::Value::as_str)
        .ok_or("schedule foundation: config.default must be a string")?;
    stable_id("config default agent", default)?;
    let agents = root
        .get("agents")
        .and_then(toml::Value::as_array)
        .ok_or("schedule foundation: config template must contain one agent")?;
    if agents.len() != 1 {
        return Err("schedule foundation: config template must contain exactly one agent".into());
    }
    let agent = agents[0]
        .as_table()
        .ok_or("schedule foundation: config template agent must be a table")?;
    exact_toml_keys(
        agent,
        &[
            "id",
            "kind",
            "cmd",
            "pre_authenticated",
            "model",
            "effort",
            "mode",
            "args",
            "base_url",
            "api_key_env",
            "sandbox",
        ],
        "agent",
    )?;
    let agent_id = optional_toml_string(agent, "id", "agent")?
        .ok_or("schedule foundation: agent.id is required")?;
    stable_id("config agent id", &agent_id)?;
    if agent_id != default {
        return Err("schedule foundation: config default and sole agent id disagree".into());
    }
    let kind = optional_toml_string(agent, "kind", "agent")?.unwrap_or_else(|| "acp".into());
    let command_family = optional_toml_string(agent, "cmd", "agent")?;
    let model = optional_toml_string(agent, "model", "agent")?
        .ok_or("schedule foundation: agent.model is required")?;
    bounded_text("config agent model", &model)?;
    let effort = optional_toml_string(agent, "effort", "agent")?;
    let mode = optional_toml_string(agent, "mode", "agent")?;
    let base_url = optional_toml_string(agent, "base_url", "agent")?;
    let api_key_env = optional_toml_string(agent, "api_key_env", "agent")?;
    if api_key_env.as_deref().is_some_and(|name| !env_name(name)) {
        return Err("schedule foundation: config template api_key_env is invalid".into());
    }
    let pre_authenticated = agent
        .get("pre_authenticated")
        .map(|value| {
            value.as_bool().ok_or_else(|| -> BoxError {
                "schedule foundation: agent.pre_authenticated must be a boolean".into()
            })
        })
        .transpose()?
        .unwrap_or(false);
    let args = toml_string_array(agent, "args", "agent")?;
    let allowed_cwd_root = optional_toml_string(root, "allowed_cwd_root", "config")?;
    let sandbox = agent
        .get("sandbox")
        .map(|value| -> Result<CanonicalSandboxTemplateV1, BoxError> {
            let table = value
                .as_table()
                .ok_or("schedule foundation: agent.sandbox must be a table")?;
            exact_toml_keys(
                table,
                &[
                    "image", "mount", "access", "egress", "network", "proxy", "volumes",
                ],
                "agent.sandbox",
            )?;
            let required = |field: &str| -> Result<String, BoxError> {
                optional_toml_string(table, field, "agent.sandbox")?.ok_or_else(|| {
                    format!("schedule foundation: agent.sandbox.{field} is required").into()
                })
            };
            let image_family = image_family
                .ok_or("schedule foundation: sandbox config requires an image-family binding")?;
            let image = required("image")?;
            if !image.starts_with("sha256:") || !local_file::valid_sha256(&image[7..]) {
                return Err(
                    "schedule foundation: sandbox image must be an immutable digest".into(),
                );
            }
            let mut volumes = toml_string_array(table, "volumes", "agent.sandbox")?;
            volumes.sort();
            if volumes.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err("schedule foundation: sandbox volumes must be unique".into());
            }
            Ok(CanonicalSandboxTemplateV1 {
                image_family: image_family.to_owned(),
                mount: required("mount")?,
                access: required("access")?,
                egress: required("egress")?,
                network: required("network")?,
                proxy: required("proxy")?,
                volumes,
            })
        })
        .transpose()?;
    if sandbox.is_none() && image_family.is_some() {
        return Err("schedule foundation: image-family binding requires a sandbox config".into());
    }
    let mut allowed_commands = root
        .get("registry")
        .map(|value| {
            let table = value
                .as_table()
                .ok_or("schedule foundation: config.registry must be a table")?;
            exact_toml_keys(table, &["allowed_cmds"], "registry")?;
            toml_string_array(table, "allowed_cmds", "registry")
        })
        .transpose()?
        .unwrap_or_default();
    let server = root
        .get("server")
        .and_then(toml::Value::as_table)
        .ok_or("schedule foundation: config.server must be a table")?;
    exact_toml_keys(server, &["addr"], "server")?;
    let server_addr = optional_toml_string(server, "addr", "server")?
        .ok_or("schedule foundation: server.addr is required")?;
    bounded_text("server.addr", &server_addr)?;
    allowed_commands.sort();
    if allowed_commands.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("schedule foundation: registry allowed commands must be unique".into());
    }
    let projection = CanonicalConfigTemplateV1 {
        schema_version: 1,
        template_id: template_id.to_owned(),
        default_agent: default.to_owned(),
        agent_id,
        kind,
        command_family,
        model,
        effort,
        mode,
        base_url,
        api_key_env,
        pre_authenticated,
        args,
        allowed_cwd_root,
        sandbox,
        allowed_commands,
        server_addr,
    };
    let hash = canonical_hash("config template", &projection)?;
    Ok((hash, projection))
}

fn validate_policy(policy: &SchedulePolicyV1) -> Result<(), BoxError> {
    if policy.schema_version != 1 {
        return Err("schedule foundation: policy schema_version must be 1".into());
    }
    stable_id("policy.environment_owner", &policy.environment_owner)?;
    if policy.trusted_cwd_root != Path::new(OWNER_APPROVED_TRUSTED_CWD_ROOT) {
        return Err(
            "schedule foundation: trusted_cwd_root does not match the owner-approved repository root"
                .into(),
        );
    }
    bounded_text("policy.repository", &policy.repository)?;
    stable_id(
        "policy.fixed_prompt_contract",
        &policy.fixed_prompt_contract,
    )?;
    stable_id("policy.artifact_template", &policy.artifact_template)?;
    stable_id(
        "policy.price_ranking_authority",
        &policy.price_ranking_authority,
    )?;
    if policy.price_snapshot_max_age_secs == 0
        || policy.price_snapshot_max_age_secs > 31 * 24 * 60 * 60
    {
        return Err(
            "schedule foundation: price_snapshot_max_age_secs must be in 1..=2678400".into(),
        );
    }
    for (label, path) in [
        ("scheduled_registry", &policy.scheduled_registry),
        (
            "characterization_inventory",
            &policy.characterization_inventory,
        ),
        ("production_manifest", &policy.production_manifest),
        ("floating_recipes", &policy.floating_recipes),
    ] {
        validate_relative_path(label, path)?;
    }
    let triggers = policy
        .allowed_triggers
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if triggers.len() != policy.allowed_triggers.len()
        || triggers
            != BTreeSet::from([
                TriggerKindV1::ManualCharacterization,
                TriggerKindV1::ManualCompatibility,
                TriggerKindV1::Daily,
                TriggerKindV1::ScheduledMain,
                TriggerKindV1::TestMerge,
            ])
    {
        return Err(
            "schedule foundation: policy must name each approved trigger exactly once".into(),
        );
    }
    let effects = policy
        .allowed_effects
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if effects.len() != policy.allowed_effects.len()
        || effects
            != BTreeSet::from([
                EffectClassV1::ProviderPrompt,
                EffectClassV1::RegistryRead,
                EffectClassV1::ImageInspect,
                EffectClassV1::ImageBuild,
            ])
    {
        return Err(
            "schedule foundation: policy must name each approved effect exactly once".into(),
        );
    }
    policy.profile_maxima.validate("policy.profile_maxima")?;
    if policy.deferred_profiles.len() > MAX_DEFERRED_PROFILES {
        return Err("schedule foundation: too many deferred profile records".into());
    }
    if policy
        .deferred_profiles
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != policy.deferred_profiles.len()
    {
        return Err("schedule foundation: deferred profile records must be unique".into());
    }
    for (index, value) in policy.deferred_profiles.iter().enumerate() {
        bounded_text(&format!("deferred_profiles[{index}]"), value)?;
    }
    validate_storage_policy(&policy.storage)?;
    Ok(())
}

fn validate_storage_policy(storage: &StoragePolicyV1) -> Result<(), BoxError> {
    if storage.hot_root != "~/Library/Application Support/a2a-bridge/operator/evidence"
        || storage.cold_root != "~/Documents/a2a-bridge/evidence-archive"
    {
        return Err(
            "schedule foundation: storage roots do not match the owner-approved paths".into(),
        );
    }
    if storage.hot_cap_bytes != 10 * 1024 * 1024 * 1024
        || storage.hot_index_cap_bytes != 1024 * 1024 * 1024
        || storage.hot_scratch_cap_bytes != 4 * 1024 * 1024 * 1024
        || storage.hot_sealed_cap_bytes != 5 * 1024 * 1024 * 1024
        || storage.hot_index_cap_bytes
            + storage.hot_scratch_cap_bytes
            + storage.hot_sealed_cap_bytes
            != storage.hot_cap_bytes
        || storage.cold_cap_bytes != 25 * 1024 * 1024 * 1024
    {
        return Err(
            "schedule foundation: storage caps do not match the approved 10/25 GiB allocation"
                .into(),
        );
    }
    Ok(())
}

fn validate_registry(
    registry: &ScheduledCaseRegistryV1,
    policy: &SchedulePolicyV1,
) -> Result<(), BoxError> {
    if registry.schema_version != 1 {
        return Err("schedule foundation: scheduled registry schema_version must be 1".into());
    }
    if registry.cases.is_empty() || registry.cases.len() > MAX_CASES {
        return Err(format!(
            "schedule foundation: scheduled registry must contain 1..={MAX_CASES} cases"
        )
        .into());
    }
    let mut ids = BTreeSet::new();
    for case in &registry.cases {
        validate_scheduled_case(case, policy)?;
        if !ids.insert(case.id.as_str()) {
            return Err(format!(
                "schedule foundation: duplicate scheduled case id {:?}",
                case.id
            )
            .into());
        }
    }
    Ok(())
}

fn validate_scheduled_case(
    case: &ScheduledCaseV1,
    policy: &SchedulePolicyV1,
) -> Result<(), BoxError> {
    stable_id("scheduled case id", &case.id)?;
    stable_id("scheduled provider_family", &case.provider_family)?;
    stable_id("scheduled capability", &case.capability)?;
    stable_id("scheduled agent", &case.agent)?;
    bounded_text("scheduled model", &case.model)?;
    bounded_text(
        "scheduled expected_effective_model",
        &case.expected_effective_model,
    )?;
    if case.model != case.expected_effective_model
        || case.effort != case.expected_effective_effort
        || case.mode != case.expected_effective_mode
    {
        return Err(format!(
            "schedule foundation: case {:?} proposed and expected-effective identities must initially match",
            case.id
        )
        .into());
    }
    if case.evidence_purpose != EvidencePurposeV1::ProviderPathAdvisory {
        return Err(format!(
            "schedule foundation: case {:?} is not a provider-path advisory profile",
            case.id
        )
        .into());
    }
    if case.evidence_path != "bridge_smoke"
        || case.probe != "minimal"
        || case.expected_status != ExpectedStatusV1::Pass
    {
        return Err(format!(
            "schedule foundation: case {:?} must use the reviewed bridge_smoke/minimal/PASS contract",
            case.id
        )
        .into());
    }
    if case.environment_owner != policy.environment_owner {
        return Err(format!(
            "schedule foundation: case {:?} has the wrong environment owner",
            case.id
        )
        .into());
    }
    if case.os != "macos" || case.architecture != "aarch64" {
        return Err(format!(
            "schedule foundation: case {:?} has an unsupported environment",
            case.id
        )
        .into());
    }
    resolve_trusted_session_cwd(
        &format!("case {:?} session cwd", case.id),
        &case.session_cwd,
        &policy.trusted_cwd_root,
    )?;
    validate_relative_path("scheduled case config", &case.config)?;
    stable_id("scheduled config_template", &case.config_template)?;
    bounded_text("scheduled adapter_family", &case.adapter_family)?;
    bounded_text("scheduled agent_cli_family", &case.agent_cli_family)?;
    if let Some(image_family) = &case.image_family {
        stable_id("scheduled image_family", image_family)?;
    }
    if case.required_env.len() > MAX_REQUIRED_ENV {
        return Err(format!(
            "schedule foundation: case {:?} has too many required environment entries",
            case.id
        )
        .into());
    }
    let mut envs = BTreeSet::new();
    for required in &case.required_env {
        if !env_name(&required.name) || !envs.insert(required.name.as_str()) {
            return Err(format!(
                "schedule foundation: case {:?} has an invalid or duplicate required environment name",
                case.id
            )
            .into());
        }
        if compatibility::credential_shaped_env_name(&required.name) {
            return Err(format!(
                "schedule foundation: case {:?} must declare credential-shaped environment name {:?} as credential_env, not required_env",
                case.id, required.name
            )
            .into());
        }
        if case.credential_env.as_deref() == Some(required.name.as_str()) {
            return Err(format!(
                "schedule foundation: case {:?} must not repeat credential_env in required_env",
                case.id
            )
            .into());
        }
        let values = required.one_of.iter().collect::<BTreeSet<_>>();
        if values.len() != required.one_of.len() {
            return Err(format!(
                "schedule foundation: case {:?} repeats a required environment value",
                case.id
            )
            .into());
        }
        for value in &required.one_of {
            bounded_text("required environment value", value)?;
        }
    }
    match (&case.auth_path, &case.credential_env) {
        (ScheduleAuthPathV1::ApiKeyEnv, Some(name)) if env_name(name) => {}
        (ScheduleAuthPathV1::ApiKeyEnv, _) => {
            return Err(format!(
                "schedule foundation: API-key case {:?} requires one credential environment name",
                case.id
            )
            .into())
        }
        (_, None) => {}
        _ => {
            return Err(format!(
                "schedule foundation: non-API-key case {:?} must not declare credential_env",
                case.id
            )
            .into())
        }
    }
    let effects = case
        .allowed_effects
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if effects.len() != case.allowed_effects.len()
        || !effects.contains(&EffectClassV1::ProviderPrompt)
        || !effects.is_subset(&policy.allowed_effects.iter().copied().collect())
    {
        return Err(format!(
            "schedule foundation: case {:?} has invalid or duplicate effect classes",
            case.id
        )
        .into());
    }
    let expected_effects = expected_effects(case.execution_mode);
    if effects != expected_effects {
        return Err(format!(
            "schedule foundation: case {:?} effect classes do not match its execution mode",
            case.id
        )
        .into());
    }
    match case.execution_mode {
        ScheduleExecutionModeV1::ContainerRo if case.image_family.is_none() => {
            return Err(format!(
                "schedule foundation: reader case {:?} requires an image family",
                case.id
            )
            .into())
        }
        ScheduleExecutionModeV1::Host | ScheduleExecutionModeV1::RemoteApi
            if case.image_family.is_some() =>
        {
            return Err(format!(
                "schedule foundation: non-reader case {:?} must not declare an image family",
                case.id
            )
            .into())
        }
        _ => {}
    }
    if case.execution_mode == ScheduleExecutionModeV1::RemoteApi && case.resolution_case.is_some() {
        return Err(format!(
            "schedule foundation: static API case {:?} must not claim an R3c resolution case",
            case.id
        )
        .into());
    }
    if let Some(resolution_case) = &case.resolution_case {
        stable_id("scheduled resolution_case", resolution_case)?;
    }
    case.caps.validate(&format!("case {:?} caps", case.id))?;
    case.caps
        .within(&policy.profile_maxima, &format!("case {:?} caps", case.id))?;
    if case.artifact.retention_days == 0 || case.artifact.retention_days > 365 {
        return Err(format!(
            "schedule foundation: case {:?} retention_days must be in 1..=365",
            case.id
        )
        .into());
    }
    Ok(())
}

fn expected_effects(mode: ScheduleExecutionModeV1) -> BTreeSet<EffectClassV1> {
    match mode {
        ScheduleExecutionModeV1::Host => {
            BTreeSet::from([EffectClassV1::ProviderPrompt, EffectClassV1::RegistryRead])
        }
        ScheduleExecutionModeV1::ContainerRo => BTreeSet::from([
            EffectClassV1::ProviderPrompt,
            EffectClassV1::RegistryRead,
            EffectClassV1::ImageInspect,
            EffectClassV1::ImageBuild,
        ]),
        ScheduleExecutionModeV1::RemoteApi => BTreeSet::from([EffectClassV1::ProviderPrompt]),
    }
}

fn validate_config_cross_bindings(
    case: &ScheduledCaseV1,
    config: &CanonicalConfigTemplateV1,
) -> Result<(), BoxError> {
    let (expected_kind, expected_command, expected_adapter, expected_cli) =
        match case.provider_family.as_str() {
            "openai-codex" => (
                "acp",
                Some("codex-acp"),
                "@agentclientprotocol/codex-acp",
                "@openai/codex",
            ),
            "anthropic-claude" => (
                "acp",
                Some("claude-agent-acp"),
                "@agentclientprotocol/claude-agent-acp",
                "@anthropic-ai/claude-agent-sdk",
            ),
            "ollama-local" => ("api", None, "bridge-api-openai-compatible", "ollama-server"),
            provider => {
                return Err(format!(
                    "schedule foundation: case {:?} uses unknown provider family {provider:?}",
                    case.id
                )
                .into())
            }
        };
    if config.kind != expected_kind
        || config.command_family.as_deref() != expected_command
        || case.adapter_family != expected_adapter
        || case.agent_cli_family != expected_cli
    {
        return Err(format!(
            "schedule foundation: case {:?} provider/adapter/command families disagree",
            case.id
        )
        .into());
    }
    if config.server_addr != "127.0.0.1:8080" {
        return Err(format!(
            "schedule foundation: case {:?} must keep the inert loopback bridge server binding",
            case.id
        )
        .into());
    }
    let expected_base_url =
        (case.provider_family == "ollama-local").then_some("http://127.0.0.1:11434/v1");
    if config.base_url.as_deref() != expected_base_url {
        return Err(format!(
            "schedule foundation: case {:?} provider endpoint contradicts its reviewed provider path",
            case.id
        )
        .into());
    }
    let expected_args = match (case.provider_family.as_str(), case.execution_mode) {
        ("openai-codex", ScheduleExecutionModeV1::Host) => vec![
            "-c".to_owned(),
            "sandbox_mode=\"read-only\"".to_owned(),
            "-c".to_owned(),
            "approval_policy=\"never\"".to_owned(),
        ],
        ("openai-codex", ScheduleExecutionModeV1::ContainerRo) => vec![
            "-c".to_owned(),
            "sandbox_mode=\"danger-full-access\"".to_owned(),
            "-c".to_owned(),
            "approval_policy=\"never\"".to_owned(),
        ],
        _ => Vec::new(),
    };
    if config.args != expected_args {
        return Err(format!(
            "schedule foundation: case {:?} command arguments contradict its reviewed effect boundary",
            case.id
        )
        .into());
    }
    match case.execution_mode {
        ScheduleExecutionModeV1::ContainerRo => {
            let sandbox = config.sandbox.as_ref().ok_or_else(|| -> BoxError {
                format!(
                    "schedule foundation: reader case {:?} has no canonical sandbox",
                    case.id
                )
                .into()
            })?;
            let expected_volume = match case.provider_family.as_str() {
                "openai-codex" => {
                    "/Users/wesleyjinks/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json"
                }
                "anthropic-claude" => "/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json",
                _ => {
                    return Err(format!(
                        "schedule foundation: reader case {:?} has no reviewed credential-volume contract",
                        case.id
                    )
                    .into())
                }
            };
            if config.allowed_cwd_root.as_deref() != Some(OWNER_APPROVED_TRUSTED_CWD_ROOT)
                || sandbox.mount != OWNER_APPROVED_TRUSTED_CWD_ROOT
                || sandbox.access != "ro"
                || sandbox.egress != "locked"
                || sandbox.network != "a2a-egress-internal"
                || sandbox.proxy != "http://a2a-egress-proxy:8888"
                || sandbox.volumes != [expected_volume]
            {
                return Err(format!(
                    "schedule foundation: reader case {:?} sandbox/mount/egress/proxy/credential-volume contract drifted",
                    case.id
                )
                .into());
            }
        }
        ScheduleExecutionModeV1::Host | ScheduleExecutionModeV1::RemoteApi => {
            if config.allowed_cwd_root.is_some() || config.sandbox.is_some() {
                return Err(format!(
                    "schedule foundation: non-reader case {:?} must not carry a container filesystem boundary",
                    case.id
                )
                .into());
            }
        }
    }
    let auth_matches = match case.auth_path {
        ScheduleAuthPathV1::PreAuthenticated => {
            config.pre_authenticated
                && config.api_key_env.is_none()
                && case.credential_env.is_none()
        }
        ScheduleAuthPathV1::Automatic => {
            !config.pre_authenticated
                && config.api_key_env.is_none()
                && case.credential_env.is_none()
        }
        ScheduleAuthPathV1::ApiKeyEnv => {
            !config.pre_authenticated
                && config.api_key_env.as_deref() == case.credential_env.as_deref()
        }
    };
    if !auth_matches {
        return Err(format!(
            "schedule foundation: case {:?} auth/pre-auth/API-key bindings disagree",
            case.id
        )
        .into());
    }
    let expected_commands = match case.execution_mode {
        ScheduleExecutionModeV1::Host => expected_command.into_iter().collect::<Vec<_>>(),
        ScheduleExecutionModeV1::ContainerRo => vec!["docker"],
        ScheduleExecutionModeV1::RemoteApi => Vec::new(),
    };
    if config.allowed_commands
        != expected_commands
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    {
        return Err(format!(
            "schedule foundation: case {:?} registry command allowance disagrees",
            case.id
        )
        .into());
    }
    Ok(())
}

fn validate_config(
    root: &Path,
    case: &ScheduledCaseV1,
) -> Result<(String, toml::Value, CapturedFoundationFile), BoxError> {
    let path = root.join(&case.config);
    let snapshot = local_file::read_regular_file_bounded(
        &path,
        &format!("scheduled config {:?}", case.id),
        MAX_CONFIG_BYTES,
    )?;
    ensure_snapshot_within_root(
        root,
        &snapshot.canonical_path,
        &format!("scheduled config {:?}", case.id),
    )?;
    let text = std::str::from_utf8(&snapshot.bytes)
        .map_err(|_| format!("schedule foundation: config {:?} must be UTF-8", case.id))?;
    if compatibility::looks_like_secret(text) {
        return Err(format!(
            "schedule foundation: config {:?} contains secret-shaped material",
            case.id
        )
        .into());
    }
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| format!("schedule foundation: invalid config {:?}: {error}", case.id))?;
    let table = value.as_table().ok_or_else(|| {
        format!(
            "schedule foundation: config {:?} must be a TOML table",
            case.id
        )
    })?;
    if table.get("default").and_then(toml::Value::as_str) != Some(case.agent.as_str()) {
        return Err(format!(
            "schedule foundation: config {:?} default agent does not match the registry",
            case.id
        )
        .into());
    }
    let agents = table
        .get("agents")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| {
            format!(
                "schedule foundation: config {:?} must have one agent",
                case.id
            )
        })?;
    if agents.len() != 1 {
        return Err(format!(
            "schedule foundation: config {:?} must have exactly one agent",
            case.id
        )
        .into());
    }
    let agent = agents[0].as_table().ok_or_else(|| {
        format!(
            "schedule foundation: config {:?} agent must be a table",
            case.id
        )
    })?;
    let field = |name: &str| agent.get(name).and_then(toml::Value::as_str);
    if field("id") != Some(case.agent.as_str())
        || field("model") != Some(case.model.as_str())
        || field("effort") != case.effort.as_deref()
        || field("mode") != case.mode.as_deref()
    {
        return Err(format!(
            "schedule foundation: config {:?} agent/model/effort/mode differs from its registry row",
            case.id
        )
        .into());
    }
    let kind = field("kind").unwrap_or("acp");
    match case.execution_mode {
        ScheduleExecutionModeV1::RemoteApi if kind != "api" || agent.contains_key("sandbox") => {
            return Err(format!(
                "schedule foundation: remote API config {:?} must be an unsandboxed API agent",
                case.id
            )
            .into())
        }
        ScheduleExecutionModeV1::ContainerRo => {
            let sandbox = agent
                .get("sandbox")
                .and_then(toml::Value::as_table)
                .ok_or_else(|| {
                    format!(
                        "schedule foundation: reader config {:?} requires a sandbox",
                        case.id
                    )
                })?;
            if kind != "acp"
                || sandbox.get("access").and_then(toml::Value::as_str) != Some("ro")
                || !sandbox
                    .get("image")
                    .and_then(toml::Value::as_str)
                    .is_some_and(|image| {
                        image.starts_with("sha256:")
                            && image.len() == 71
                            && image[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            {
                return Err(format!(
                    "schedule foundation: reader config {:?} must use ACP, read-only access, and an immutable image",
                    case.id
                )
                .into());
            }
        }
        ScheduleExecutionModeV1::Host if kind != "acp" || agent.contains_key("sandbox") => {
            return Err(format!(
                "schedule foundation: host config {:?} must be an unsandboxed ACP agent",
                case.id
            )
            .into())
        }
        _ => {}
    }
    let (template_sha256, config_projection) = canonical_config_template_hash(
        &case.config_template,
        case.image_family.as_deref(),
        &value,
    )?;
    validate_config_cross_bindings(case, &config_projection)?;
    let captured = CapturedFoundationFile {
        canonical_path: snapshot.canonical_path,
        sha256: snapshot.sha256,
        file_identity: snapshot.identity,
        label: format!("scheduled config {:?}", case.id),
        max_bytes: MAX_CONFIG_BYTES,
    };
    Ok((template_sha256, value, captured))
}

fn scheduled_profile(
    policy: &SchedulePolicyV1,
    case: &ScheduledCaseV1,
    config_template_sha256: String,
    exact_config_sha256: String,
    resolution_constraint_sha256: Option<String>,
) -> Result<CanonicalProfileInputV1, BoxError> {
    let mut allowed_effects = case.allowed_effects.clone();
    allowed_effects.sort();
    let required_env = canonical_required_environment(case.required_env.clone());
    let session_cwd = resolve_trusted_session_cwd(
        &format!("case {:?} session cwd", case.id),
        &case.session_cwd,
        &policy.trusted_cwd_root,
    )?;
    Ok(CanonicalProfileInputV1 {
        schema_version: 1,
        source_kind: ProfileSourceKindV1::ScheduledAdvisory,
        source_id: case.id.clone(),
        repository: policy.repository.clone(),
        source_schema_version: 1,
        lane: "floating-current".into(),
        classification: "canary".into(),
        evidence_purpose: case.evidence_purpose,
        evidence_path: case.evidence_path.clone(),
        probe: case.probe.clone(),
        expected_status: case.expected_status,
        execution_mode: match case.execution_mode {
            ScheduleExecutionModeV1::Host => "host",
            ScheduleExecutionModeV1::ContainerRo => "container_ro",
            ScheduleExecutionModeV1::RemoteApi => "remote_api",
        }
        .into(),
        provider_family: case.provider_family.clone(),
        agent: case.agent.clone(),
        capability: case.capability.clone(),
        adapter_family: case.adapter_family.clone(),
        agent_cli_family: case.agent_cli_family.clone(),
        image_family: case.image_family.clone(),
        auth_path: match case.auth_path {
            ScheduleAuthPathV1::ApiKeyEnv => "api_key_env",
            ScheduleAuthPathV1::PreAuthenticated => "pre_authenticated",
            ScheduleAuthPathV1::Automatic => "automatic",
        }
        .into(),
        credential_env_name: case.credential_env.clone(),
        required_env,
        environment_owner: case.environment_owner.clone(),
        os: case.os.clone(),
        architecture: case.architecture.clone(),
        session_cwd: session_cwd.to_string_lossy().into_owned(),
        requested_model: case.model.clone(),
        requested_effort: case.effort.clone(),
        requested_mode: case.mode.clone(),
        expected_effective_model: case.expected_effective_model.clone(),
        expected_effective_effort: case.expected_effective_effort.clone(),
        expected_effective_mode: case.expected_effective_mode.clone(),
        config_template: case.config_template.clone(),
        config_template_sha256,
        exact_config_sha256,
        resolution_constraint_sha256,
        allowed_effects,
        fixed_prompt_contract: policy.fixed_prompt_contract.clone(),
        artifact_template: policy.artifact_template.clone(),
        artifact_retention_days: case.artifact.retention_days,
        artifact_redaction: "strict".into(),
        maximum_caps: case.caps.clone(),
    })
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalRecipeConstraintV1<'a> {
    schema_version: u16,
    case: &'a compatibility_resolution::FloatingCaseRecipe,
    package_set: &'a compatibility_resolution::PackageSetRecipe,
    image: Option<CanonicalRecipeImageV1<'a>>,
    limits: &'a compatibility_resolution::ResolutionLimits,
    artifact: &'a compatibility_resolution::ResolutionArtifactPolicy,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalRecipeImageV1<'a> {
    id: &'a str,
    template: compatibility_resolution::ImageTemplate,
    base: &'a str,
    package_sets: Vec<&'a str>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalSupportResolutionConstraintV1<'a> {
    schema_version: u16,
    policy: &'static str,
    adapter_family: &'a str,
    agent_cli_family: &'a str,
    image_family: Option<&'a str>,
}

fn config_template_id(value: compatibility_resolution::ConfigTemplate) -> &'static str {
    match value {
        compatibility_resolution::ConfigTemplate::CodexHostReadOnlyV1 => "codex-host-read-only-v1",
        compatibility_resolution::ConfigTemplate::CodexReaderReadOnlyV1 => {
            "codex-reader-read-only-v1"
        }
        compatibility_resolution::ConfigTemplate::ClaudeHostReadOnlyV1 => {
            "claude-host-read-only-v1"
        }
        compatibility_resolution::ConfigTemplate::ClaudeReaderReadOnlyV1 => {
            "claude-reader-read-only-v1"
        }
    }
}

fn recipe_constraint_sha256(
    recipes: &compatibility_resolution::FloatingRecipeManifest,
    case: &ScheduledCaseV1,
) -> Result<Option<String>, BoxError> {
    let Some(resolution_case) = &case.resolution_case else {
        if case.execution_mode != ScheduleExecutionModeV1::RemoteApi {
            return Err(format!(
                "schedule foundation: case {:?} lacks its required resolution case",
                case.id
            )
            .into());
        }
        return Ok(None);
    };
    let recipe = recipes
        .cases
        .iter()
        .find(|recipe| &recipe.id == resolution_case)
        .ok_or_else(|| {
            format!(
                "schedule foundation: case {:?} references unknown resolution case {:?}",
                case.id, resolution_case
            )
        })?;
    let package_set = recipes
        .package_sets
        .iter()
        .find(|package| package.id == recipe.package_set)
        .ok_or_else(|| {
            format!(
                "schedule foundation: resolution case {:?} has no package set",
                recipe.id
            )
        })?;
    let image = recipe
        .image
        .as_ref()
        .map(|image_id| {
            recipes
                .images
                .iter()
                .find(|image| &image.id == image_id)
                .ok_or_else(|| {
                    format!(
                        "schedule foundation: resolution case {:?} has no image",
                        recipe.id
                    )
                })
        })
        .transpose()?;
    let canonical_image = image.map(|image| {
        let mut package_sets = image
            .package_sets
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        package_sets.sort();
        CanonicalRecipeImageV1 {
            id: &image.id,
            template: image.template,
            base: &image.base,
            package_sets,
        }
    });
    let expected_target = match case.execution_mode {
        ScheduleExecutionModeV1::Host => compatibility_resolution::FloatingTarget::HostPackageTree,
        ScheduleExecutionModeV1::ContainerRo => {
            compatibility_resolution::FloatingTarget::ContainerRoImage
        }
        ScheduleExecutionModeV1::RemoteApi => {
            return Err(format!(
                "schedule foundation: API case {:?} must not use a resolution recipe",
                case.id
            )
            .into())
        }
    };
    if recipe.target != expected_target
        || config_template_id(recipe.config_template) != case.config_template
        || package_set.adapter != case.adapter_family
        || package_set.agent_cli != case.agent_cli_family
        || recipe.image.as_deref() != case.image_family.as_deref().map(|_| "reader-current")
    {
        return Err(format!(
            "schedule foundation: case {:?} and its semantic resolution recipe disagree",
            case.id
        )
        .into());
    }
    canonical_hash(
        "profile resolution constraint",
        &CanonicalRecipeConstraintV1 {
            schema_version: 1,
            case: recipe,
            package_set,
            image: canonical_image,
            limits: &recipes.limits,
            artifact: &recipes.artifact,
        },
    )
    .map(Some)
}

fn support_profiles(
    policy: &SchedulePolicyV1,
    root: &Path,
    manifest_bytes: &[u8],
    captures: &mut Vec<CapturedFoundationFile>,
) -> Result<Vec<CanonicalProfileInputV1>, BoxError> {
    let text = std::str::from_utf8(manifest_bytes)
        .map_err(|_| "schedule foundation: production manifest must be UTF-8")?;
    let manifest: toml::Value = toml::from_str(text)
        .map_err(|error| format!("schedule foundation: invalid production manifest: {error}"))?;
    let cases = manifest
        .get("cases")
        .and_then(toml::Value::as_array)
        .ok_or("schedule foundation: production manifest has no cases")?;
    let default_max_cost_usd = manifest
        .get("budget")
        .and_then(toml::Value::as_table)
        .and_then(|budget| budget.get("max_cost_usd"))
        .and_then(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|value| value as f64))
        })
        .ok_or("schedule foundation: production manifest has no cost ceiling")?;
    let mut result = Vec::new();
    for case in cases {
        let table = case
            .as_table()
            .ok_or("schedule foundation: production case must be a table")?;
        if table.get("classification").and_then(toml::Value::as_str) != Some("support") {
            continue;
        }
        result.push(support_profile(
            policy,
            root,
            table,
            default_max_cost_usd,
            captures,
        )?);
    }
    Ok(result)
}

fn support_profile(
    policy: &SchedulePolicyV1,
    root: &Path,
    case: &toml::map::Map<String, toml::Value>,
    default_max_cost_usd: f64,
    captures: &mut Vec<CapturedFoundationFile>,
) -> Result<CanonicalProfileInputV1, BoxError> {
    let required = |name: &str| -> Result<String, BoxError> {
        case.get(name)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("schedule foundation: support case missing {name}").into())
    };
    let optional = |name: &str| {
        case.get(name)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    let id = required("id")?;
    let environment_owner = required("environment_owner")?;
    let os = required("os")?;
    let architecture = required("architecture")?;
    if environment_owner != policy.environment_owner || os != "macos" || architecture != "aarch64" {
        return Err(format!(
            "schedule foundation: support case {id:?} does not match the owner-approved environment"
        )
        .into());
    }
    let session_cwd = optional("session_cwd").ok_or_else(|| -> BoxError {
        format!("schedule foundation: support case {id:?} has no trusted session cwd").into()
    })?;
    let session_cwd = resolve_trusted_session_cwd(
        &format!("support case {id:?} session cwd"),
        Path::new(&session_cwd),
        &policy.trusted_cwd_root,
    )?
    .to_string_lossy()
    .into_owned();
    let config = PathBuf::from(required("config")?);
    validate_relative_path("support config", &config)?;
    let config_snapshot = local_file::read_regular_file_bounded(
        &root.join(&config),
        &format!("support config {id:?}"),
        MAX_CONFIG_BYTES,
    )?;
    ensure_snapshot_within_root(
        root,
        &config_snapshot.canonical_path,
        &format!("support config {id:?}"),
    )?;
    let config_text = std::str::from_utf8(&config_snapshot.bytes)
        .map_err(|_| format!("schedule foundation: support config {id:?} must be UTF-8"))?;
    if compatibility::looks_like_secret(config_text) {
        return Err(format!(
            "schedule foundation: support config {id:?} contains secret-shaped material"
        )
        .into());
    }
    let exact_config_sha256 = config_snapshot.sha256.clone();
    captures.push(CapturedFoundationFile {
        canonical_path: config_snapshot.canonical_path.clone(),
        sha256: config_snapshot.sha256.clone(),
        file_identity: config_snapshot.identity.clone(),
        label: format!("support config {id:?}"),
        max_bytes: MAX_CONFIG_BYTES,
    });
    let config_value: toml::Value = toml::from_str(config_text)
        .map_err(|error| format!("schedule foundation: invalid support config {id:?}: {error}"))?;
    let pins = case
        .get("pins")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("schedule foundation: support case {id:?} has no pins"))?;
    let pinned_config_sha256 = pins
        .get("config_sha256")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            format!("schedule foundation: support case {id:?} has no config_sha256 pin")
        })?;
    validate_sha256("support config_sha256 pin", pinned_config_sha256)?;
    if pinned_config_sha256 != config_snapshot.sha256 {
        return Err(format!(
            "schedule foundation: support case {id:?} config bytes do not match its exact manifest pin"
        )
        .into());
    }
    let package_family = |name: &str| -> Result<String, BoxError> {
        let value = pins
            .get(name)
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("schedule foundation: support case {id:?} lacks {name}"))?;
        value
            .split_once('=')
            .map(|(package, _)| package.to_owned())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid {name}").into()
            })
    };
    let execution_mode = required("execution_mode")?;
    let agent = required("agent")?;
    let provider_family = if agent.starts_with("codex") {
        "openai-codex"
    } else if agent.starts_with("claude") {
        "anthropic-claude"
    } else {
        return Err(
            format!("schedule foundation: unsupported claimed-support agent {agent:?}").into(),
        );
    };
    let required_env = case
        .get("required_env")
        .cloned()
        .map(|value| value.try_into::<Vec<RequiredEnvironmentV1>>())
        .transpose()
        .map_err(|error| format!("schedule foundation: invalid support required_env: {error}"))?
        .unwrap_or_default();
    let required_env = canonical_required_environment(required_env);
    let artifact = case
        .get("artifact")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            format!("schedule foundation: support case {id:?} has no artifact policy")
        })?;
    let retention_days = artifact
        .get("retention_days")
        .and_then(toml::Value::as_integer)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| format!("schedule foundation: support case {id:?} has invalid retention"))?;
    let cost_microusd = case
        .get("max_cost_usd")
        .and_then(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|value| value as f64))
        })
        .or(Some(default_max_cost_usd))
        .map(|cost| (cost * 1_000_000.0).round() as u64)
        .expect("default production-manifest cost is required");
    let caps = EffectCapsV1 {
        timeout_secs: case
            .get("timeout_secs")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid timeout")
            })?,
        max_tokens: case
            .get("max_tokens")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid token cap")
            })?,
        max_cost_microusd: cost_microusd,
        attempts: 1,
        retry_cap: case
            .get("retry_cap")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid retry cap")
            })?,
        fallback_cap: 0,
    };
    caps.validate(&format!("support case {id:?} caps"))?;
    caps.within(&policy.profile_maxima, &format!("support case {id:?} caps"))?;
    let model = required("model")?;
    let adapter_family = package_family("adapter")?;
    let agent_cli_family = package_family("agent_cli")?;
    let config_template = match (provider_family, execution_mode.as_str()) {
        ("openai-codex", "host") => "codex-host-read-only-v1",
        ("openai-codex", "container_ro") => "codex-reader-read-only-v1",
        ("anthropic-claude", "host") => "claude-host-read-only-v1",
        ("anthropic-claude", "container_ro") => "claude-reader-read-only-v1",
        _ => {
            return Err(format!(
                "schedule foundation: support case {id:?} has no approved config template"
            )
            .into())
        }
    };
    let image_family = (execution_mode == "container_ro").then(|| "node-acp-reader-v1".into());
    let (config_template_sha256, config_projection) =
        canonical_config_template_hash(config_template, image_family.as_deref(), &config_value)?;
    let config_root = config_value
        .as_table()
        .ok_or_else(|| format!("schedule foundation: support config {id:?} is not a table"))?;
    let config_agent = config_root
        .get("agents")
        .and_then(toml::Value::as_array)
        .and_then(|agents| agents.first())
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("schedule foundation: support config {id:?} has no agent"))?;
    let config_field = |name: &str| config_agent.get(name).and_then(toml::Value::as_str);
    let expected_command = match provider_family {
        "openai-codex" => "codex-acp",
        "anthropic-claude" => "claude-agent-acp",
        _ => unreachable!("provider family was closed above"),
    };
    let auth_path = required("auth_path")?;
    let expected_pre_authenticated = auth_path == "pre_authenticated";
    let expected_commands = if execution_mode == "container_ro" {
        vec!["docker".to_owned()]
    } else {
        vec![expected_command.to_owned()]
    };
    if config_root.get("default").and_then(toml::Value::as_str) != Some(agent.as_str())
        || config_field("id") != Some(agent.as_str())
        || config_field("model") != Some(model.as_str())
        || config_field("effort") != optional("effort").as_deref()
        || config_field("mode") != optional("mode").as_deref()
        || config_projection.kind != "acp"
        || config_projection.command_family.as_deref() != Some(expected_command)
        || config_projection.pre_authenticated != expected_pre_authenticated
        || config_projection.api_key_env.is_some()
        || config_projection.allowed_commands != expected_commands
        || !matches!(auth_path.as_str(), "automatic" | "pre_authenticated")
    {
        return Err(format!(
            "schedule foundation: support case {id:?} config/provider/auth bindings disagree"
        )
        .into());
    }
    validate_claimed_support_config_effect_boundary(
        &id,
        provider_family,
        &execution_mode,
        &config_projection,
    )?;
    let expected_status = match required("expected_status")?.as_str() {
        "PASS" => ExpectedStatusV1::Pass,
        "FAIL" => ExpectedStatusV1::Fail,
        "UNKNOWN" => ExpectedStatusV1::Unknown,
        "STALE" => ExpectedStatusV1::Stale,
        value => {
            return Err(format!(
                "schedule foundation: support case {id:?} has invalid expected_status {value:?}"
            )
            .into())
        }
    };
    let resolution_constraint_sha256 = canonical_hash(
        "claimed-support resolution constraint",
        &CanonicalSupportResolutionConstraintV1 {
            schema_version: 1,
            policy: "exact-pins-bound-only-in-case-execution",
            adapter_family: &adapter_family,
            agent_cli_family: &agent_cli_family,
            image_family: image_family.as_deref(),
        },
    )?;
    let allowed_effects = if execution_mode == "container_ro" {
        vec![EffectClassV1::ProviderPrompt, EffectClassV1::ImageInspect]
    } else {
        vec![EffectClassV1::ProviderPrompt]
    };
    Ok(CanonicalProfileInputV1 {
        schema_version: 1,
        source_kind: ProfileSourceKindV1::ClaimedSupportGate,
        source_id: id,
        repository: policy.repository.clone(),
        source_schema_version: 1,
        lane: required("lane")?,
        classification: "support".into(),
        evidence_purpose: EvidencePurposeV1::ClaimedSupportGate,
        evidence_path: required("evidence_path")?,
        probe: required("probe")?,
        expected_status,
        execution_mode: execution_mode.clone(),
        provider_family: provider_family.into(),
        agent,
        capability: "claimed-support".into(),
        adapter_family,
        agent_cli_family,
        image_family,
        auth_path,
        credential_env_name: optional("credential_env"),
        required_env,
        environment_owner,
        os,
        architecture,
        session_cwd,
        requested_model: model.clone(),
        requested_effort: optional("effort"),
        requested_mode: optional("mode"),
        expected_effective_model: model,
        expected_effective_effort: optional("effort"),
        expected_effective_mode: optional("mode"),
        config_template: config_template.into(),
        config_template_sha256,
        exact_config_sha256,
        resolution_constraint_sha256: Some(resolution_constraint_sha256),
        allowed_effects,
        fixed_prompt_contract: policy.fixed_prompt_contract.clone(),
        artifact_template: policy.artifact_template.clone(),
        artifact_retention_days: retention_days,
        artifact_redaction: artifact
            .get("redaction")
            .and_then(toml::Value::as_str)
            .unwrap_or("invalid")
            .to_owned(),
        maximum_caps: caps,
    })
}

fn validate_claimed_support_config_effect_boundary(
    id: &str,
    provider_family: &str,
    execution_mode: &str,
    config: &CanonicalConfigTemplateV1,
) -> Result<(), BoxError> {
    if config.server_addr != "127.0.0.1:8080" || config.base_url.is_some() {
        return Err(format!(
            "schedule foundation: support case {id:?} endpoint contradicts its reviewed provider path"
        )
        .into());
    }
    let expected_args = match (provider_family, execution_mode) {
        ("openai-codex", "host") => vec![
            "-c".to_owned(),
            "sandbox_mode=\"read-only\"".to_owned(),
            "-c".to_owned(),
            "approval_policy=\"never\"".to_owned(),
        ],
        ("openai-codex", "container_ro") => vec![
            "-c".to_owned(),
            "sandbox_mode=\"danger-full-access\"".to_owned(),
            "-c".to_owned(),
            "approval_policy=\"never\"".to_owned(),
        ],
        ("anthropic-claude", "host" | "container_ro") => Vec::new(),
        _ => {
            return Err(format!(
                "schedule foundation: support case {id:?} has no reviewed provider/effect boundary"
            )
            .into())
        }
    };
    if config.args != expected_args {
        return Err(format!(
            "schedule foundation: support case {id:?} command arguments contradict its reviewed effect boundary"
        )
        .into());
    }
    if execution_mode == "host" {
        if config.allowed_cwd_root.is_some() || config.sandbox.is_some() {
            return Err(format!(
                "schedule foundation: support host case {id:?} must not carry a container filesystem boundary"
            )
            .into());
        }
        return Ok(());
    }

    let sandbox = config.sandbox.as_ref().ok_or_else(|| -> BoxError {
        format!("schedule foundation: support reader case {id:?} has no canonical sandbox").into()
    })?;
    let mut expected_volumes = match provider_family {
        "openai-codex" => vec![
            "/Users/wesleyjinks/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json"
                .to_owned(),
        ],
        "anthropic-claude" => vec![
            "/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json"
                .to_owned(),
            "/Users/wesleyjinks/code/a2a-bridge-operator-main/deploy/containers/claude-fable-settings.json:/root/.claude/settings.json:ro"
                .to_owned(),
        ],
        _ => unreachable!("provider family was closed above"),
    };
    expected_volumes.sort();
    if config.allowed_cwd_root.as_deref() != Some(OWNER_APPROVED_TRUSTED_CWD_ROOT)
        || sandbox.mount != OWNER_APPROVED_TRUSTED_CWD_ROOT
        || sandbox.access != "ro"
        || sandbox.egress != "locked"
        || sandbox.network != "a2a-egress-internal"
        || sandbox.proxy != "http://a2a-egress-proxy:8888"
        || sandbox.volumes != expected_volumes
    {
        return Err(format!(
            "schedule foundation: support reader case {id:?} sandbox/mount/egress/proxy/credential-volume contract drifted"
        )
        .into());
    }
    Ok(())
}

fn canonical_required_environment(
    mut values: Vec<RequiredEnvironmentV1>,
) -> Vec<RequiredEnvironmentV1> {
    for required in &mut values {
        required.one_of.sort();
    }
    values.sort_by(|left, right| left.name.cmp(&right.name));
    values
}

fn validate_inventory(
    inventory: &CharacterizationProfileInventoryV1,
    expected: &BTreeMap<(ProfileSourceKindV1, String), String>,
) -> Result<(), BoxError> {
    if inventory.schema_version != 1 {
        return Err(
            "schedule foundation: characterization inventory schema_version must be 1".into(),
        );
    }
    let mut observed = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for profile in &inventory.profiles {
        stable_id("inventory profile id", &profile.id)?;
        stable_id("inventory source id", &profile.source_id)?;
        validate_sha256("inventory profile_sha256", &profile.profile_sha256)?;
        if profile.id != profile.source_id {
            return Err(format!(
                "schedule foundation: inventory profile {:?} must retain its exact source id",
                profile.id
            )
            .into());
        }
        if !ids.insert(profile.id.as_str()) {
            return Err(format!(
                "schedule foundation: duplicate inventory id {:?}",
                profile.id
            )
            .into());
        }
        let key = (profile.source_kind, profile.source_id.clone());
        if observed
            .insert(key, profile.profile_sha256.clone())
            .is_some()
        {
            return Err("schedule foundation: duplicate inventory source reference".into());
        }
    }
    if observed.keys().collect::<BTreeSet<_>>() != expected.keys().collect::<BTreeSet<_>>() {
        let missing = expected
            .keys()
            .filter(|key| !observed.contains_key(*key))
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>();
        let extra = observed
            .keys()
            .filter(|key| !expected.contains_key(*key))
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>();
        return Err(format!(
            "schedule foundation: characterization inventory mismatch (missing={missing:?}, extra={extra:?})"
        )
        .into());
    }
    let mismatches = expected
        .iter()
        .filter_map(|(key, expected_hash)| {
            let actual = observed.get(key)?;
            (actual != expected_hash).then(|| {
                format!(
                    "{}:{} expected {} but found {}",
                    match key.0 {
                        ProfileSourceKindV1::ScheduledAdvisory => "scheduled_advisory",
                        ProfileSourceKindV1::ClaimedSupportGate => "claimed_support_gate",
                    },
                    key.1,
                    expected_hash,
                    actual
                )
            })
        })
        .collect::<Vec<_>>();
    if !mismatches.is_empty() {
        return Err(format!(
            "schedule foundation: characterization fingerprint mismatch: {}",
            mismatches.join("; ")
        )
        .into());
    }
    Ok(())
}

fn canonical_policy_sha256(policy: &SchedulePolicyV1) -> Result<String, BoxError> {
    let mut value = policy.clone();
    value.allowed_triggers.sort();
    value.allowed_effects.sort();
    value.deferred_profiles.sort();
    canonical_hash("scheduling policy", &value)
}

fn canonical_floating_recipes_sha256(
    recipes: &compatibility_resolution::FloatingRecipeManifest,
) -> Result<String, BoxError> {
    let mut value = recipes.clone();
    value
        .package_sets
        .sort_by(|left, right| left.id.cmp(&right.id));
    value.images.sort_by(|left, right| left.id.cmp(&right.id));
    for image in &mut value.images {
        image.package_sets.sort();
    }
    value.cases.sort_by(|left, right| left.id.cmp(&right.id));
    canonical_hash("floating recipe constraints", &value)
}

fn recheck_foundation_files(captures: &[CapturedFoundationFile]) -> Result<(), BoxError> {
    recheck_foundation_files_with_hook(captures, |_| {})
}

fn recheck_foundation_files_with_hook<F>(
    captures: &[CapturedFoundationFile],
    mut after_recheck: F,
) -> Result<(), BoxError>
where
    F: FnMut(usize),
{
    let mut seen = BTreeMap::<&Path, (&str, &local_file::RegularFileIdentity)>::new();
    for (index, capture) in captures.iter().enumerate() {
        if let Some((previous_sha256, previous_identity)) = seen.insert(
            &capture.canonical_path,
            (&capture.sha256, &capture.file_identity),
        ) {
            if previous_sha256 != capture.sha256 || previous_identity != &capture.file_identity {
                return Err(format!(
                    "schedule foundation: {:?} was observed with conflicting identities",
                    capture.canonical_path
                )
                .into());
            }
            continue;
        }
        let current = local_file::read_regular_file_bounded(
            &capture.canonical_path,
            &format!("{} final recheck", capture.label),
            capture.max_bytes,
        )?;
        if current.canonical_path != capture.canonical_path
            || current.sha256 != capture.sha256
            || current.identity != capture.file_identity
        {
            return Err(format!(
                "schedule foundation: {} changed during the multi-file snapshot",
                capture.label
            )
            .into());
        }
        after_recheck(index);
    }
    Ok(())
}

pub(super) fn load_schedule_foundation(root: &Path) -> Result<LoadedScheduleFoundation, BoxError> {
    if root.as_os_str().is_empty() {
        return Err("schedule foundation: root must be non-empty".into());
    }
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("schedule foundation: cannot canonicalize root: {error}"))?;
    if !root.is_dir() {
        return Err("schedule foundation: root must be a directory".into());
    }
    let planned_root = local_file::snapshot_directory(&root, "schedule foundation root")?;
    let pinned_root = local_file::PinnedDirectory::open(
        &root,
        &planned_root.canonical_cwd,
        &planned_root.identity,
        "schedule foundation root",
    )?;
    let root = pinned_root.canonical_path();
    let policy = load_foundation_toml::<SchedulePolicyV1>(
        &root,
        &root.join("scheduling-policy.toml"),
        "scheduling policy",
    )?;
    validate_policy(&policy.value)?;
    let policy_semantic_sha256 = canonical_policy_sha256(&policy.value)?;
    let registry = load_foundation_toml::<ScheduledCaseRegistryV1>(
        &root,
        &root.join(&policy.value.scheduled_registry),
        "scheduled case registry",
    )?;
    validate_registry(&registry.value, &policy.value)?;
    let inventory = load_foundation_toml::<CharacterizationProfileInventoryV1>(
        &root,
        &root.join(&policy.value.characterization_inventory),
        "characterization profile inventory",
    )?;
    let mut captures = vec![
        CapturedFoundationFile {
            canonical_path: policy.canonical_path.clone(),
            sha256: policy.sha256.clone(),
            file_identity: policy.file_identity.clone(),
            label: "scheduling policy".into(),
            max_bytes: MAX_FOUNDATION_FILE_BYTES,
        },
        CapturedFoundationFile {
            canonical_path: registry.canonical_path.clone(),
            sha256: registry.sha256.clone(),
            file_identity: registry.file_identity.clone(),
            label: "scheduled case registry".into(),
            max_bytes: MAX_FOUNDATION_FILE_BYTES,
        },
        CapturedFoundationFile {
            canonical_path: inventory.canonical_path.clone(),
            sha256: inventory.sha256.clone(),
            file_identity: inventory.file_identity.clone(),
            label: "characterization profile inventory".into(),
            max_bytes: MAX_FOUNDATION_FILE_BYTES,
        },
    ];

    let production_manifest_snapshot =
        compatibility::validated_manifest_snapshot(&root.join(&policy.value.production_manifest))?;
    ensure_snapshot_within_root(
        &root,
        &production_manifest_snapshot.canonical_path,
        "production manifest",
    )?;
    captures.push(CapturedFoundationFile {
        canonical_path: production_manifest_snapshot.canonical_path.clone(),
        sha256: production_manifest_snapshot.sha256.clone(),
        file_identity: production_manifest_snapshot.identity.clone(),
        label: "production manifest".into(),
        max_bytes: MAX_FOUNDATION_FILE_BYTES,
    });
    let production_manifest_sha256 = production_manifest_snapshot.sha256.clone();
    let production_manifest_bytes = production_manifest_snapshot.bytes;
    let floating =
        compatibility_resolution::load_recipes(&root.join(&policy.value.floating_recipes))?;
    ensure_snapshot_within_root(&root, &floating.canonical_path, "floating recipes")?;
    if floating.recipes.production_manifest != policy.value.production_manifest {
        return Err(
            "schedule foundation: floating recipes and policy name different production manifests"
                .into(),
        );
    }
    let floating_recipes_semantic_sha256 = canonical_floating_recipes_sha256(&floating.recipes)?;
    captures.push(CapturedFoundationFile {
        canonical_path: floating.canonical_path.clone(),
        sha256: floating.sha256.clone(),
        file_identity: floating
            .file_identity
            .clone()
            .ok_or("schedule foundation: floating recipe loader omitted file identity")?,
        label: "floating recipes".into(),
        max_bytes: MAX_FOUNDATION_FILE_BYTES,
    });

    let mut expected = BTreeMap::new();
    let mut scheduled_hashes = BTreeMap::new();
    let mut scheduled_profiles = BTreeMap::new();
    let mut config_hashes = BTreeMap::new();
    for case in &registry.value.cases {
        let (config_sha256, _, capture) = validate_config(&root, case)?;
        let exact_config_sha256 = capture.sha256.clone();
        captures.push(capture);
        config_hashes.insert(
            case.config.to_string_lossy().into_owned(),
            config_sha256.clone(),
        );
        let resolution_constraint_sha256 = recipe_constraint_sha256(&floating.recipes, case)?;
        let profile = scheduled_profile(
            &policy.value,
            case,
            config_sha256,
            exact_config_sha256,
            resolution_constraint_sha256,
        )?;
        let hash = canonical_hash("scheduled characterization profile", &profile)?;
        let row_sha256 = canonical_hash("scheduled source row", case)?;
        let binding = foundation_profile_binding(
            &profile,
            SchemaProfileSourceKindV1::ScheduledAdvisory,
            registry.sha256.clone(),
            row_sha256,
            hash.clone(),
        );
        expected.insert(
            (ProfileSourceKindV1::ScheduledAdvisory, case.id.clone()),
            hash.clone(),
        );
        scheduled_hashes.insert(case.id.clone(), hash);
        scheduled_profiles.insert(case.id.clone(), binding);
    }

    let support = support_profiles(
        &policy.value,
        &root,
        &production_manifest_bytes,
        &mut captures,
    )?;
    let support_ids = support
        .iter()
        .map(|profile| profile.source_id.as_str())
        .collect::<BTreeSet<_>>();
    if support_ids != EXPECTED_SUPPORT_PROFILES.into_iter().collect() {
        return Err(format!(
            "schedule foundation: claimed-support inventory changed: {support_ids:?}"
        )
        .into());
    }
    let mut support_hashes = BTreeMap::new();
    let mut claimed_support_profiles = BTreeMap::new();
    for profile in support {
        config_hashes.insert(
            profile.config_template.clone(),
            profile.config_template_sha256.clone(),
        );
        let hash = canonical_hash("claimed-support characterization profile", &profile)?;
        let row_sha256 = canonical_hash("claimed-support source row", &profile)?;
        let binding = foundation_profile_binding(
            &profile,
            SchemaProfileSourceKindV1::ClaimedSupportGate,
            production_manifest_sha256.clone(),
            row_sha256,
            hash.clone(),
        );
        expected.insert(
            (
                ProfileSourceKindV1::ClaimedSupportGate,
                profile.source_id.clone(),
            ),
            hash.clone(),
        );
        claimed_support_profiles.insert(profile.source_id.clone(), binding);
        support_hashes.insert(profile.source_id, hash);
    }
    validate_inventory(&inventory.value, &expected)?;

    let registry_semantic_sha256 = canonical_hash("scheduled profile set", &scheduled_hashes)?;
    let inventory_projection = expected
        .iter()
        .map(|((kind, id), hash)| {
            let kind = match kind {
                ProfileSourceKindV1::ScheduledAdvisory => "scheduled_advisory",
                ProfileSourceKindV1::ClaimedSupportGate => "claimed_support_gate",
            };
            (format!("{kind}:{id}"), hash.clone())
        })
        .collect::<BTreeMap<_, _>>();
    let inventory_semantic_sha256 =
        canonical_hash("characterization profile inventory", &inventory_projection)?;

    let bundle = ProfilePolicyBundleInputV1 {
        schema_version: 1,
        policy_sha256: policy_semantic_sha256,
        registry_sha256: registry_semantic_sha256,
        inventory_sha256: inventory_semantic_sha256,
        floating_recipes_sha256: floating_recipes_semantic_sha256,
        scheduled_profiles: scheduled_hashes.clone(),
        claimed_support_profiles: support_hashes.clone(),
        config_templates: config_hashes,
        allowed_effects: policy.value.allowed_effects,
        profile_maxima: policy.value.profile_maxima,
        fixed_prompt_contract: policy.value.fixed_prompt_contract,
        artifact_template: policy.value.artifact_template,
    };
    let bundle_sha256 = canonical_hash("profile policy bundle", &bundle)?;
    recheck_foundation_files(&captures)?;
    if !pinned_root.current_path_matches() {
        return Err("schedule foundation: root identity changed during validation".into());
    }
    Ok(LoadedScheduleFoundation {
        scheduled_profile_count: scheduled_hashes.len(),
        claimed_support_profile_count: support_hashes.len(),
        profile_policy_bundle_sha256: bundle_sha256,
        scheduled_profiles,
        claimed_support_profiles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility")
    }

    #[test]
    fn checked_in_foundation_has_no_effecting_entrypoint_and_exact_profile_sets() {
        let loaded = load_schedule_foundation(&fixture_root()).unwrap();
        assert_eq!(loaded.scheduled_profile_count, 6);
        assert_eq!(loaded.claimed_support_profile_count, 4);
        assert!(local_file::valid_sha256(
            &loaded.profile_policy_bundle_sha256
        ));
    }

    #[test]
    fn caps_reject_retry_fallback_and_profile_maximum_overrun() {
        let maximum = EffectCapsV1 {
            timeout_secs: 100,
            max_tokens: 100,
            max_cost_microusd: 100,
            attempts: 1,
            retry_cap: 0,
            fallback_cap: 0,
        };
        let retry = EffectCapsV1 {
            retry_cap: 1,
            ..maximum.clone()
        };
        assert!(retry
            .validate("retry")
            .unwrap_err()
            .to_string()
            .contains("retry/fallback zero"));
        let widened = EffectCapsV1 {
            timeout_secs: 101,
            ..maximum.clone()
        };
        assert!(widened.within(&maximum, "widened").is_err());
    }

    #[test]
    fn required_environment_identity_is_set_order_independent() {
        let left = vec![
            RequiredEnvironmentV1 {
                name: "Z_ENV".into(),
                one_of: vec!["two".into(), "one".into()],
            },
            RequiredEnvironmentV1 {
                name: "A_ENV".into(),
                one_of: vec!["beta".into(), "alpha".into()],
            },
        ];
        let right = vec![
            RequiredEnvironmentV1 {
                name: "A_ENV".into(),
                one_of: vec!["alpha".into(), "beta".into()],
            },
            RequiredEnvironmentV1 {
                name: "Z_ENV".into(),
                one_of: vec!["one".into(), "two".into()],
            },
        ];
        assert_eq!(
            canonical_required_environment(left),
            canonical_required_environment(right)
        );
    }

    #[test]
    fn canonical_hashes_are_domain_separated() {
        let value = vec!["same", "canonical", "bytes"];
        let profile = canonical_hash("scheduled characterization profile", &value).unwrap();
        let bundle = canonical_hash("profile policy bundle", &value).unwrap();
        assert_ne!(profile, bundle);
    }

    #[test]
    fn every_canonical_profile_field_changes_recharacterization_identity() {
        let sha = |ch: char| ch.to_string().repeat(64);
        let profile = CanonicalProfileInputV1 {
            schema_version: 1,
            source_kind: ProfileSourceKindV1::ScheduledAdvisory,
            source_id: "case-1".into(),
            repository: "shoedog/a2acp".into(),
            source_schema_version: 1,
            lane: "floating-current".into(),
            classification: "canary".into(),
            evidence_purpose: EvidencePurposeV1::ProviderPathAdvisory,
            evidence_path: "host".into(),
            probe: "health".into(),
            expected_status: ExpectedStatusV1::Pass,
            execution_mode: "host".into(),
            provider_family: "openai-codex".into(),
            agent: "codex-host".into(),
            capability: "provider-path".into(),
            adapter_family: "codex-acp".into(),
            agent_cli_family: "codex".into(),
            image_family: Some("node-acp-reader-v1".into()),
            auth_path: "pre_authenticated".into(),
            credential_env_name: Some("OPENAI_API_KEY".into()),
            required_env: vec![RequiredEnvironmentV1 {
                name: "PATH".into(),
                one_of: vec!["/opt/homebrew/bin".into()],
            }],
            environment_owner: "wesleyjinks".into(),
            os: "macos".into(),
            architecture: "aarch64".into(),
            session_cwd: "/Users/wesleyjinks/code/a2a-bridge".into(),
            requested_model: "gpt-5.6-luna".into(),
            requested_effort: Some("low".into()),
            requested_mode: None,
            expected_effective_model: "gpt-5.6-luna".into(),
            expected_effective_effort: Some("low".into()),
            expected_effective_mode: None,
            config_template: "codex-host-read-only-v1".into(),
            config_template_sha256: sha('1'),
            exact_config_sha256: sha('2'),
            resolution_constraint_sha256: Some(sha('3')),
            allowed_effects: vec![EffectClassV1::ProviderPrompt],
            fixed_prompt_contract: "compatibility-smoke-v1".into(),
            artifact_template: "compatibility-evidence-v1".into(),
            artifact_retention_days: 180,
            artifact_redaction: "strict".into(),
            maximum_caps: EffectCapsV1 {
                timeout_secs: 60,
                max_tokens: 1000,
                max_cost_microusd: 1000,
                attempts: 1,
                retry_cap: 0,
                fallback_cap: 0,
            },
        };
        let base = canonical_hash("scheduled characterization profile", &profile).unwrap();
        macro_rules! changes_profile {
            ($name:literal, |$value:ident| $body:block) => {{
                let mut $value = profile.clone();
                $body
                assert_ne!(
                    canonical_hash("scheduled characterization profile", &$value).unwrap(),
                    base,
                    "{}",
                    $name
                );
            }};
        }
        changes_profile!("schema_version", |value| {
            value.schema_version = 2;
        });
        changes_profile!("source_kind", |value| {
            value.source_kind = ProfileSourceKindV1::ClaimedSupportGate;
        });
        changes_profile!("source_id", |value| {
            value.source_id = "case-2".into();
        });
        changes_profile!("repository", |value| {
            value.repository = "shoedog/other".into();
        });
        changes_profile!("source_schema_version", |value| {
            value.source_schema_version = 2;
        });
        changes_profile!("lane", |value| {
            value.lane = "support".into();
        });
        changes_profile!("classification", |value| {
            value.classification = "support".into();
        });
        changes_profile!("evidence_purpose", |value| {
            value.evidence_purpose = EvidencePurposeV1::ClaimedSupportGate;
        });
        changes_profile!("evidence_path", |value| {
            value.evidence_path = "reader".into();
        });
        changes_profile!("probe", |value| {
            value.probe = "catalog".into();
        });
        changes_profile!("expected_status", |value| {
            value.expected_status = ExpectedStatusV1::Fail;
        });
        changes_profile!("execution_mode", |value| {
            value.execution_mode = "container_ro".into();
        });
        changes_profile!("provider_family", |value| {
            value.provider_family = "anthropic-claude".into();
        });
        changes_profile!("agent", |value| {
            value.agent = "codex-reader".into();
        });
        changes_profile!("capability", |value| {
            value.capability = "claimed-support".into();
        });
        changes_profile!("adapter_family", |value| {
            value.adapter_family = "claude-agent-acp".into();
        });
        changes_profile!("agent_cli_family", |value| {
            value.agent_cli_family = "claude".into();
        });
        changes_profile!("image_family", |value| {
            value.image_family = None;
        });
        changes_profile!("auth_path", |value| {
            value.auth_path = "automatic".into();
        });
        changes_profile!("credential_env_name", |value| {
            value.credential_env_name = None;
        });
        changes_profile!("required_env_name", |value| {
            value.required_env[0].name = "HOME".into();
        });
        changes_profile!("required_env_constraint", |value| {
            value.required_env[0].one_of[0] = "/usr/bin".into();
        });
        changes_profile!("environment_owner", |value| {
            value.environment_owner = "other".into();
        });
        changes_profile!("os", |value| {
            value.os = "linux".into();
        });
        changes_profile!("architecture", |value| {
            value.architecture = "x86_64".into();
        });
        changes_profile!("session_cwd", |value| {
            value.session_cwd = "/Users/wesleyjinks/code/stockTrading".into();
        });
        changes_profile!("requested_model", |value| {
            value.requested_model = "gpt-5.6-sol".into();
        });
        changes_profile!("requested_effort", |value| {
            value.requested_effort = Some("medium".into());
        });
        changes_profile!("requested_mode", |value| {
            value.requested_mode = Some("plan".into());
        });
        changes_profile!("expected_effective_model", |value| {
            value.expected_effective_model = "gpt-5.6-sol".into();
        });
        changes_profile!("expected_effective_effort", |value| {
            value.expected_effective_effort = Some("medium".into());
        });
        changes_profile!("expected_effective_mode", |value| {
            value.expected_effective_mode = Some("plan".into());
        });
        changes_profile!("config_template", |value| {
            value.config_template = "codex-reader-read-only-v1".into();
        });
        changes_profile!("config_template_sha256", |value| {
            value.config_template_sha256 = sha('4');
        });
        changes_profile!("resolution_constraint_sha256", |value| {
            value.resolution_constraint_sha256 = Some(sha('5'));
        });
        changes_profile!("allowed_effects", |value| {
            value.allowed_effects = vec![EffectClassV1::RegistryRead];
        });
        changes_profile!("fixed_prompt_contract", |value| {
            value.fixed_prompt_contract = "compatibility-smoke-v2".into();
        });
        changes_profile!("artifact_template", |value| {
            value.artifact_template = "compatibility-evidence-v2".into();
        });
        changes_profile!("artifact_retention_days", |value| {
            value.artifact_retention_days += 1;
        });
        changes_profile!("artifact_redaction", |value| {
            value.artifact_redaction = "strict-v2".into();
        });
        changes_profile!("maximum_timeout", |value| {
            value.maximum_caps.timeout_secs += 1;
        });
        changes_profile!("maximum_tokens", |value| {
            value.maximum_caps.max_tokens += 1;
        });
        changes_profile!("maximum_cost", |value| {
            value.maximum_caps.max_cost_microusd += 1;
        });
        changes_profile!("maximum_attempts", |value| {
            value.maximum_caps.attempts += 1;
        });
        changes_profile!("maximum_retry", |value| {
            value.maximum_caps.retry_cap += 1;
        });
        changes_profile!("maximum_fallback", |value| {
            value.maximum_caps.fallback_cap += 1;
        });

        let mut execution_only = profile;
        execution_only.exact_config_sha256 = sha('6');
        assert_eq!(
            canonical_hash("scheduled characterization profile", &execution_only).unwrap(),
            base,
            "exact config bytes are deliberately case-execution identity, not profile identity"
        );
    }

    #[test]
    fn trusted_session_cwd_rejects_traversal_relative_and_sibling_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("trusted-root");
        let inside = root.join("repository");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir(&outside).unwrap();

        assert_eq!(
            resolve_trusted_session_cwd("valid fixture", &inside, &root).unwrap(),
            std::fs::canonicalize(&inside).unwrap()
        );
        assert_eq!(
            resolve_trusted_session_cwd("root fixture", &root, &root).unwrap(),
            std::fs::canonicalize(&root).unwrap()
        );

        for path in [
            root.join("../outside"),
            temp.path().join("trusted-root-sibling/repository"),
            PathBuf::from("relative/repo"),
        ] {
            assert!(resolve_trusted_session_cwd("invalid fixture", &path, &root).is_err());
        }

        #[cfg(unix)]
        {
            let link = root.join("outside-link");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            let error = resolve_trusted_session_cwd("symlink escape fixture", &link, &root)
                .unwrap_err()
                .to_string();
            assert!(error.contains("owner-approved trusted cwd root"), "{error}");
        }
    }

    #[test]
    fn trusted_session_cwd_retains_static_identity_when_owner_root_is_offline() {
        let temp = tempfile::tempdir().unwrap();
        let offline_root = temp.path().join("owner-root-not-mounted");
        let declared = offline_root.join("repository");
        assert_eq!(
            resolve_trusted_session_cwd("offline fixture", &declared, &offline_root).unwrap(),
            declared
        );
    }

    #[test]
    fn final_snapshot_recheck_rejects_mid_validation_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("foundation.toml");
        std::fs::write(&path, b"schema_version = 1\n").unwrap();
        let snapshot = local_file::read_regular_file_bounded(
            &path,
            "foundation recheck fixture",
            MAX_FOUNDATION_FILE_BYTES,
        )
        .unwrap();
        let capture = CapturedFoundationFile {
            canonical_path: snapshot.canonical_path,
            sha256: snapshot.sha256,
            file_identity: snapshot.identity,
            label: "foundation recheck fixture".into(),
            max_bytes: MAX_FOUNDATION_FILE_BYTES,
        };
        recheck_foundation_files(std::slice::from_ref(&capture)).unwrap();
        std::fs::write(&path, b"schema_version = 2\n").unwrap();
        assert!(recheck_foundation_files(&[capture])
            .unwrap_err()
            .to_string()
            .contains("changed during"));
    }

    #[test]
    fn final_snapshot_recheck_rejects_same_content_object_swap_and_mixed_generation() {
        let temp = tempfile::tempdir().unwrap();
        let first_path = temp.path().join("first.toml");
        let second_path = temp.path().join("second.toml");
        std::fs::write(&first_path, b"schema_version = 1\n").unwrap();
        std::fs::write(&second_path, b"schema_version = 1\n").unwrap();
        let capture = |path: &Path, label: &str| {
            let snapshot =
                local_file::read_regular_file_bounded(path, label, MAX_FOUNDATION_FILE_BYTES)
                    .unwrap();
            CapturedFoundationFile {
                canonical_path: snapshot.canonical_path,
                sha256: snapshot.sha256,
                file_identity: snapshot.identity,
                label: label.into(),
                max_bytes: MAX_FOUNDATION_FILE_BYTES,
            }
        };
        let captures = vec![
            capture(&first_path, "first fixture"),
            capture(&second_path, "second fixture"),
        ];
        let displaced = temp.path().join("second-old.toml");
        let error = recheck_foundation_files_with_hook(&captures, |index| {
            if index == 0 {
                std::fs::rename(&second_path, &displaced).unwrap();
                std::fs::write(&second_path, b"schema_version = 1\n").unwrap();
            }
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("changed during"), "{error}");
    }

    #[test]
    fn final_snapshot_recheck_rejects_duplicate_path_with_conflicting_object_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("foundation.toml");
        let first_name = temp.path().join("first-object.toml");
        std::fs::write(&path, b"schema_version = 1\n").unwrap();
        let capture = |label: &str| {
            let snapshot =
                local_file::read_regular_file_bounded(&path, label, MAX_FOUNDATION_FILE_BYTES)
                    .unwrap();
            CapturedFoundationFile {
                canonical_path: snapshot.canonical_path,
                sha256: snapshot.sha256,
                file_identity: snapshot.identity,
                label: label.into(),
                max_bytes: MAX_FOUNDATION_FILE_BYTES,
            }
        };
        let first = capture("first observation");
        std::fs::rename(&path, &first_name).unwrap();
        std::fs::write(&path, b"schema_version = 1\n").unwrap();
        let second = capture("second observation");

        let chronological_error = recheck_foundation_files(&[first.clone(), second.clone()])
            .unwrap_err()
            .to_string();
        assert!(
            chronological_error.contains("changed during"),
            "{chronological_error}"
        );

        let error = recheck_foundation_files(&[second, first])
            .unwrap_err()
            .to_string();
        assert!(error.contains("conflicting identities"), "{error}");

        let same_object_first = capture("same object first observation");
        let same_object_second = capture("same object second observation");
        recheck_foundation_files(&[same_object_first, same_object_second]).unwrap();
    }
}
